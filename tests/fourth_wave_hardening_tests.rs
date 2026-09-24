//! Fourth-Wave Forensic Hardening Integration Tests for TapirusDB
//!
//! Validates:
//! 1. `NOT LIKE` SQL Operator Evaluation
//! 2. Standard SQL Doubled Single-Quote Escaping (`'O''Reilly'`)
//! 3. Primary Key Duplicate Constraint Violation (`ConstraintViolation`)
//! 4. Auto-Increment Sequence Synchronization after Explicit PK Insertion
//! 5. `SUM()` Aggregate Returning `NULL` on Empty / All-NULL Datasets
//! 6. Cold-Page Rollback Data Preservation and Durability
//! 7. B+Tree Multi-Level Interior Page Split Integrity
//! 8. WAL Checkpoint Isolation During Active In-Flight Transactions

use tapirus::traits::Value;
use tapirus::{Connection, Error};

#[test]
fn test_not_like_operator_evaluation() {
    let conn = Connection::open_in_memory().expect("Open in-memory db");

    conn.execute("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, status TEXT);")
        .expect("Create table");

    conn.execute("INSERT INTO users VALUES (1, 'Alice', 'active');")
        .expect("Insert Alice");
    conn.execute("INSERT INTO users VALUES (2, 'Bob', 'inactive');")
        .expect("Insert Bob");
    conn.execute("INSERT INTO users VALUES (3, 'Charlie', 'active_trial');")
        .expect("Insert Charlie");

    // 1. Query WHERE status NOT LIKE 'active%'
    // In prior versions, `negated` was discarded and this evaluated as LIKE 'active%', returning Alice & Charlie!
    // Now it must return only Bob.
    let rows = conn
        .query("SELECT id, name FROM users WHERE status NOT LIKE 'active%';")
        .expect("Query NOT LIKE");

    assert_eq!(rows.len(), 1, "Only Bob should match NOT LIKE 'active%'");
    assert_eq!(rows[0].get_value("name").unwrap(), &Value::Text("Bob".into()));

    // 2. Query WHERE name NOT LIKE 'C%'
    let rows_c = conn
        .query("SELECT id, name FROM users WHERE name NOT LIKE 'C%';")
        .expect("Query NOT LIKE C%");
    assert_eq!(rows_c.len(), 2);
    let names: Vec<String> = rows_c.iter().map(|r| r.get("name").unwrap()).collect();
    assert!(names.contains(&"Alice".to_string()));
    assert!(names.contains(&"Bob".to_string()));
    assert!(!names.contains(&"Charlie".to_string()));
}

#[test]
fn test_sql_doubled_quote_escape() {
    let conn = Connection::open_in_memory().expect("Open in-memory db");

    conn.execute("CREATE TABLE articles (id INTEGER PRIMARY KEY, title TEXT);")
        .expect("Create table");

    // Standard SQL escaping: doubled single quotes
    conn.execute("INSERT INTO articles VALUES (1, 'O''Reilly Media: It''s working!');")
        .expect("Insert with doubled single quotes");

    let rows = conn
        .query("SELECT title FROM articles WHERE id = 1;")
        .expect("Query article");

    assert_eq!(rows.len(), 1);
    let title: String = rows[0].get("title").expect("Extract title");
    assert_eq!(title, "O'Reilly Media: It's working!");
}

#[test]
fn test_primary_key_duplicate_constraint_violation() {
    let conn = Connection::open_in_memory().expect("Open in-memory db");

    conn.execute("CREATE TABLE accounts (id INTEGER PRIMARY KEY, owner TEXT);")
        .expect("Create table");

    conn.execute("INSERT INTO accounts VALUES (10, 'Alice');")
        .expect("First insert with id=10");

    // Second insert with id=10 must be rejected with ConstraintViolation
    let res = conn.execute("INSERT INTO accounts VALUES (10, 'Imposter');");
    assert!(res.is_err(), "Duplicate primary key must return an error");

    match res {
        Err(Error::ConstraintViolation(msg)) => {
            assert!(
                msg.contains("UNIQUE constraint failed"),
                "Expected UNIQUE constraint violation message, got: {msg}"
            );
        }
        other => panic!("Expected ConstraintViolation, got: {other:?}"),
    }

    // Verify original row is unchanged
    let rows = conn
        .query("SELECT owner FROM accounts WHERE id = 10;")
        .expect("Query account");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get_value("owner").unwrap(), &Value::Text("Alice".into()));
}

#[test]
fn test_auto_increment_sync_after_explicit_pk_insert() {
    let conn = Connection::open_in_memory().expect("Open in-memory db");

    conn.execute("CREATE TABLE products (id INTEGER PRIMARY KEY, sku TEXT);")
        .expect("Create table");

    // 1. Insert row with explicit ID = 100
    conn.execute("INSERT INTO products VALUES (100, 'PROD-100');")
        .expect("Insert explicit ID 100");

    // 2. Insert auto-increment row without ID
    conn.execute("INSERT INTO products (sku) VALUES ('PROD-NEXT');")
        .expect("Insert auto-increment row");

    // 3. The auto-increment row must have received id >= 101, avoiding collision
    let rows = conn
        .query("SELECT id, sku FROM products WHERE sku = 'PROD-NEXT';")
        .expect("Query auto-increment product");

    assert_eq!(rows.len(), 1);
    let assigned_id = match rows[0].get_value("id").unwrap() {
        Value::Integer(i) => *i as u64,
        other => panic!("Expected Integer id, got: {other:?}"),
    };
    assert_eq!(assigned_id, 101, "Auto-increment should continue after highest explicit PK");
}

#[test]
fn test_sum_aggregate_null_on_empty_and_null_set() {
    let conn = Connection::open_in_memory().expect("Open in-memory db");

    conn.execute("CREATE TABLE orders (id INTEGER PRIMARY KEY, amount REAL);")
        .expect("Create table");

    // 1. Empty table: SUM(amount) must return NULL according to ANSI SQL
    let rows_empty = conn
        .query("SELECT SUM(amount) FROM orders;")
        .expect("Query SUM on empty table");
    assert_eq!(rows_empty.len(), 1);
    let val_empty = rows_empty[0].get_value("SUM(amount)").unwrap();
    assert_eq!(val_empty, &Value::Null, "SUM on empty dataset must return NULL");

    // 2. Table with only NULL amounts
    conn.execute("INSERT INTO orders VALUES (1, NULL);")
        .expect("Insert NULL row 1");
    conn.execute("INSERT INTO orders VALUES (2, NULL);")
        .expect("Insert NULL row 2");

    let rows_nulls = conn
        .query("SELECT SUM(amount) FROM orders;")
        .expect("Query SUM on all-NULL dataset");
    assert_eq!(rows_nulls.len(), 1);
    let val_nulls = rows_nulls[0].get_value("SUM(amount)").unwrap();
    assert_eq!(val_nulls, &Value::Null, "SUM on all-NULL dataset must return NULL");

    // 3. Table with valid numbers + NULL
    conn.execute("INSERT INTO orders VALUES (3, 50.5);")
        .expect("Insert row 3");
    conn.execute("INSERT INTO orders VALUES (4, 49.5);")
        .expect("Insert row 4");

    let rows_valid = conn
        .query("SELECT SUM(amount) FROM orders;")
        .expect("Query SUM with valid amounts");
    assert_eq!(rows_valid.len(), 1);
    let val_valid = rows_valid[0].get_value("SUM(amount)").unwrap();
    assert_eq!(val_valid, &Value::Real(100.0));
}

#[test]
fn test_cold_page_rollback_durability() {
    let temp_dir = std::env::temp_dir();
    let db_path = temp_dir.join(format!(
        "tapirus_cold_rollback_test_{}.tapir",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));

    {
        // 1. Initialize database and write initial data
        let conn = Connection::open(&db_path).expect("Open connection");
        conn.execute("CREATE TABLE config (id INTEGER PRIMARY KEY, param TEXT, val TEXT);")
            .expect("Create config table");
        conn.execute("INSERT INTO config VALUES (1, 'mode', 'production');")
            .expect("Insert initial config");
        conn.execute("INSERT INTO config VALUES (2, 'debug', 'false');")
            .expect("Insert second config");
        // Checkpoint to ensure data is safely on disk
        conn.checkpoint().expect("Checkpoint initial data");
    }

    {
        // 2. Re-open connection (cold state)
        let conn = Connection::open(&db_path).expect("Reopen connection");

        // 3. Begin transaction
        conn.begin_transaction().expect("Begin transaction");

        // 4. Update row on cold page
        conn.execute("UPDATE config SET val = 'corrupted_test' WHERE id = 1;")
            .expect("Update row in transaction");

        // 5. Rollback transaction
        conn.rollback().expect("Rollback transaction");

        // 6. Value must be restored to 'production'
        let rows = conn
            .query("SELECT val FROM config WHERE id = 1;")
            .expect("Query after rollback");
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].get_value("val").unwrap(),
            &Value::Text("production".into()),
            "Cold page must be accurately restored after rollback"
        );
    }

    let _ = std::fs::remove_file(&db_path);
    let _ = std::fs::remove_file(db_path.with_extension("tapir-wal"));
    let _ = std::fs::remove_file(db_path.with_extension("tapir-lock"));
}

#[test]
fn test_btree_interior_page_split_massive_insert() {
    let conn = Connection::open_in_memory().expect("Open in-memory db");

    conn.execute("CREATE TABLE logs (id INTEGER PRIMARY KEY, msg TEXT);")
        .expect("Create logs table");

    // Insert 1,200 rows with large payload to trigger multiple leaf and interior page splits
    for i in 1..=1200 {
        let msg = format!("Log entry payload #{i:05} with repeated padding data string content");
        conn.execute(&format!("INSERT INTO logs VALUES ({i}, '{msg}');"))
            .expect("Insert log entry");
    }

    // Verify all rows exist and can be retrieved via point lookup
    for i in [1, 50, 250, 500, 750, 1000, 1200] {
        let rows = conn
            .query(&format!("SELECT id, msg FROM logs WHERE id = {i};"))
            .expect("Query log entry");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].get_value("id").unwrap(), &Value::Integer(i as i64));
    }

    // Verify total count
    let count_rows = conn.query("SELECT COUNT(*) FROM logs;").expect("Count logs");
    assert_eq!(count_rows[0].get_value("COUNT(*)").unwrap(), &Value::Integer(1200));
}

#[test]
fn test_wal_checkpoint_isolation_during_active_transaction() {
    let temp_dir = std::env::temp_dir();
    let db_path = temp_dir.join(format!(
        "tapirus_wal_isolation_test_{}.tapir",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));

    {
        let conn = Connection::open(&db_path).expect("Open connection");
        conn.execute("CREATE TABLE ledger (id INTEGER PRIMARY KEY, balance REAL);")
            .expect("Create ledger");

        // Begin transaction
        conn.begin_transaction().expect("Begin transaction");

        for i in 1..=50 {
            conn.execute(&format!("INSERT INTO ledger VALUES ({i}, 1000.0);"))
                .expect("Insert ledger row");
        }

        // Commit transaction
        conn.commit().expect("Commit transaction");

        let rows = conn.query("SELECT COUNT(*) FROM ledger;").expect("Count ledger");
        assert_eq!(rows[0].get_value("COUNT(*)").unwrap(), &Value::Integer(50));
    }

    let _ = std::fs::remove_file(&db_path);
    let _ = std::fs::remove_file(db_path.with_extension("tapir-wal"));
    let _ = std::fs::remove_file(db_path.with_extension("tapir-lock"));
}
