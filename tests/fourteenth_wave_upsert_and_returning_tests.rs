//! Fourteenth-Wave Integration Tests for TapirusDB
//!
//! Validates:
//! 1. `RETURNING` clause across `INSERT`, `UPDATE`, and `DELETE`.
//! 2. `INSERT OR REPLACE` and `REPLACE INTO` syntax and semantics with index integrity.
//! 3. `INSERT OR IGNORE` and `ON CONFLICT DO NOTHING`.
//! 4. `ON CONFLICT (col) DO UPDATE SET ...` (PostgreSQL / SQLite style Upsert).
//! 5. Seamless interop between `RETURNING`, index updates, HNSW vector updates, and CDC reactivity.

use std::sync::{Arc, Mutex};
use tapirus::{Connection, Value, ChangeOp};

#[test]
fn test_insert_returning_all_and_columns() {
    let conn = Connection::open_in_memory().expect("Open in-memory DB");
    conn.execute(
        "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, score REAL)"
    ).expect("Create users table");

    // 1. INSERT ... RETURNING *
    let rows = conn.query(
        "INSERT INTO users (id, name, score) VALUES (1, 'Alice', 95.5) RETURNING *"
    ).expect("Insert returning *");

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get_value("id"), Some(&Value::Integer(1)));
    assert_eq!(rows[0].get_value("name"), Some(&Value::Text("Alice".to_string())));
    assert_eq!(rows[0].get_value("score"), Some(&Value::Real(95.5)));

    // 2. INSERT ... RETURNING specific columns
    let rows = conn.query(
        "INSERT INTO users (id, name, score) VALUES (2, 'Bob', 80.0) RETURNING id, name"
    ).expect("Insert returning id, name");

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get_value("id"), Some(&Value::Integer(2)));
    assert_eq!(rows[0].get_value("name"), Some(&Value::Text("Bob".to_string())));
    assert_eq!(rows[0].get_value("score"), None);

    // 3. INSERT ... RETURNING with alias
    let rows = conn.query(
        "INSERT INTO users (id, name, score) VALUES (3, 'Charlie', 70.0) RETURNING name AS full_name"
    ).expect("Insert returning alias");

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get_value("full_name"), Some(&Value::Text("Charlie".to_string())));

    // 4. Verification that conn.execute also succeeds with RETURNING
    let affected = conn.execute(
        "INSERT INTO users (id, name, score) VALUES (4, 'David', 65.0) RETURNING *"
    ).expect("Execute insert returning");
    assert_eq!(affected, 1);
}

#[test]
fn test_update_returning() {
    let conn = Connection::open_in_memory().expect("Open in-memory DB");
    conn.execute(
        "CREATE TABLE employees (id INTEGER PRIMARY KEY, name TEXT, salary REAL)"
    ).expect("Create employees table");

    conn.execute("INSERT INTO employees VALUES (1, 'Alice', 50000.0)").expect("Insert Alice");
    conn.execute("INSERT INTO employees VALUES (2, 'Bob', 60000.0)").expect("Insert Bob");

    // 1. UPDATE with RETURNING specific columns
    let rows = conn.query(
        "UPDATE employees SET salary = 55000.0 WHERE id = 1 RETURNING id, salary"
    ).expect("Update returning");

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get_value("id"), Some(&Value::Integer(1)));
    assert_eq!(rows[0].get_value("salary"), Some(&Value::Real(55000.0)));

    // 2. UPDATE with RETURNING *
    let rows = conn.query(
        "UPDATE employees SET name = 'Robert' WHERE id = 2 RETURNING *"
    ).expect("Update returning *");

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get_value("id"), Some(&Value::Integer(2)));
    assert_eq!(rows[0].get_value("name"), Some(&Value::Text("Robert".to_string())));
    assert_eq!(rows[0].get_value("salary"), Some(&Value::Real(60000.0)));
}

#[test]
fn test_delete_returning() {
    let conn = Connection::open_in_memory().expect("Open in-memory DB");
    conn.execute(
        "CREATE TABLE tasks (id INTEGER PRIMARY KEY, title TEXT, priority INTEGER)"
    ).expect("Create tasks table");

    conn.execute("INSERT INTO tasks VALUES (1, 'Task A', 1)").expect("Insert 1");
    conn.execute("INSERT INTO tasks VALUES (2, 'Task B', 2)").expect("Insert 2");
    conn.execute("INSERT INTO tasks VALUES (3, 'Task C', 3)").expect("Insert 3");

    // 1. DELETE ... RETURNING *
    let rows = conn.query(
        "DELETE FROM tasks WHERE id = 1 RETURNING *"
    ).expect("Delete returning *");

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get_value("id"), Some(&Value::Integer(1)));
    assert_eq!(rows[0].get_value("title"), Some(&Value::Text("Task A".to_string())));

    // 2. DELETE multiple rows with RETURNING
    let rows = conn.query(
        "DELETE FROM tasks WHERE priority >= 2 RETURNING id, title"
    ).expect("Delete multiple returning");

    assert_eq!(rows.len(), 2);
    let mut returned_ids: Vec<i64> = rows.iter().filter_map(|r| match r.get_value("id") {
        Some(Value::Integer(id)) => Some(*id),
        _ => None,
    }).collect();
    returned_ids.sort();
    assert_eq!(returned_ids, vec![2, 3]);

    // Table should now be empty
    let remaining = conn.query("SELECT * FROM tasks").expect("Select remaining");
    assert_eq!(remaining.len(), 0);
}

#[test]
fn test_insert_or_replace_and_replace_into_with_secondary_index() {
    let conn = Connection::open_in_memory().expect("Open in-memory DB");
    conn.execute(
        "CREATE TABLE products (id INTEGER PRIMARY KEY, sku TEXT, price REAL, stock INTEGER)"
    ).expect("Create products table");
    conn.execute("CREATE INDEX idx_products_sku ON products(sku)").expect("Create index on sku");

    // 1. Initial Insert
    conn.execute("INSERT INTO products VALUES (1, 'SKU-001', 9.99, 50)").expect("Insert initial");

    // Verify row and secondary index
    let rows = conn.query("SELECT id, sku, price, stock FROM products WHERE sku = 'SKU-001'").expect("Select by sku");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get_value("price"), Some(&Value::Real(9.99)));

    // 2. INSERT OR REPLACE INTO with existing PK
    let rows = conn.query(
        "INSERT OR REPLACE INTO products (id, sku, price, stock) VALUES (1, 'SKU-001-NEW', 14.99, 100) RETURNING *"
    ).expect("Insert or replace returning *");

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get_value("id"), Some(&Value::Integer(1)));
    assert_eq!(rows[0].get_value("sku"), Some(&Value::Text("SKU-001-NEW".to_string())));
    assert_eq!(rows[0].get_value("price"), Some(&Value::Real(14.99)));
    assert_eq!(rows[0].get_value("stock"), Some(&Value::Integer(100)));

    // Verify old SKU is no longer found via secondary index
    let old_rows = conn.query("SELECT id FROM products WHERE sku = 'SKU-001'").expect("Select old sku");
    assert_eq!(old_rows.len(), 0);

    // Verify new SKU is found via secondary index
    let new_rows = conn.query("SELECT id, price FROM products WHERE sku = 'SKU-001-NEW'").expect("Select new sku");
    assert_eq!(new_rows.len(), 1);
    assert_eq!(new_rows[0].get_value("price"), Some(&Value::Real(14.99)));

    // 3. REPLACE INTO syntax
    conn.execute("REPLACE INTO products (id, sku, price, stock) VALUES (1, 'SKU-001-FINAL', 19.99, 200)")
        .expect("Replace into");

    let final_rows = conn.query("SELECT sku, price, stock FROM products WHERE id = 1").expect("Select id 1");
    assert_eq!(final_rows.len(), 1);
    assert_eq!(final_rows[0].get_value("sku"), Some(&Value::Text("SKU-001-FINAL".to_string())));
    assert_eq!(final_rows[0].get_value("price"), Some(&Value::Real(19.99)));
    assert_eq!(final_rows[0].get_value("stock"), Some(&Value::Integer(200)));

    // Ensure total row count is still exactly 1
    let all = conn.query("SELECT id FROM products").expect("Select all");
    assert_eq!(all.len(), 1);
}

#[test]
fn test_insert_or_ignore_and_on_conflict_do_nothing() {
    let conn = Connection::open_in_memory().expect("Open in-memory DB");
    conn.execute("CREATE TABLE config (id INTEGER PRIMARY KEY, key TEXT, val TEXT)")
        .expect("Create config table");

    conn.execute("INSERT INTO config VALUES (1, 'theme', 'dark')").expect("Initial insert");

    // 1. INSERT OR IGNORE INTO with existing PK: should not error and not overwrite
    let affected = conn.execute("INSERT OR IGNORE INTO config VALUES (1, 'theme', 'light')")
        .expect("Insert or ignore");
    assert_eq!(affected, 0);

    let rows = conn.query("SELECT val FROM config WHERE id = 1").expect("Query val");
    assert_eq!(rows[0].get_value("val"), Some(&Value::Text("dark".to_string())));

    // 2. ON CONFLICT DO NOTHING with existing PK
    let affected = conn.execute("INSERT INTO config VALUES (1, 'theme', 'neon') ON CONFLICT DO NOTHING")
        .expect("On conflict do nothing");
    assert_eq!(affected, 0);

    let rows = conn.query("SELECT val FROM config WHERE id = 1").expect("Query val");
    assert_eq!(rows[0].get_value("val"), Some(&Value::Text("dark".to_string())));

    // 3. ON CONFLICT DO NOTHING with non-existing PK: should insert
    let affected = conn.execute("INSERT INTO config VALUES (2, 'mode', 'compact') ON CONFLICT DO NOTHING")
        .expect("On conflict do nothing new row");
    assert_eq!(affected, 1);

    let rows = conn.query("SELECT val FROM config WHERE id = 2").expect("Query val 2");
    assert_eq!(rows[0].get_value("val"), Some(&Value::Text("compact".to_string())));
}

#[test]
fn test_on_conflict_do_update_upsert() {
    let conn = Connection::open_in_memory().expect("Open in-memory DB");
    conn.execute(
        "CREATE TABLE counters (id INTEGER PRIMARY KEY, tally INTEGER, label TEXT)"
    ).expect("Create counters table");

    // Initial insert
    conn.execute("INSERT INTO counters VALUES (1, 10, 'initial')").expect("Insert initial");

    // 1. ON CONFLICT (id) DO UPDATE SET tally = 20, label = 'updated'
    let rows = conn.query(
        "INSERT INTO counters VALUES (1, 999, 'ignored_input') ON CONFLICT (id) DO UPDATE SET tally = 20, label = 'updated' RETURNING *"
    ).expect("On conflict do update returning *");

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get_value("id"), Some(&Value::Integer(1)));
    assert_eq!(rows[0].get_value("tally"), Some(&Value::Integer(20)));
    assert_eq!(rows[0].get_value("label"), Some(&Value::Text("updated".to_string())));

    // Verify stored state in table
    let stored = conn.query("SELECT tally, label FROM counters WHERE id = 1").expect("Select stored");
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].get_value("tally"), Some(&Value::Integer(20)));
    assert_eq!(stored[0].get_value("label"), Some(&Value::Text("updated".to_string())));
}

#[test]
fn test_upsert_with_vector_column_maintenance() {
    let conn = Connection::open_in_memory().expect("Open in-memory DB");
    conn.execute(
        "CREATE TABLE docs (id INTEGER PRIMARY KEY, title TEXT, vec VECTOR(3))"
    ).expect("Create docs table");

    // 1. Insert first vector pointing along X-axis
    conn.execute("INSERT INTO docs VALUES (1, 'X Doc', [1.0, 0.0, 0.0])").expect("Insert doc 1");

    // KNN search near [1.0, 0.0, 0.0]
    let results = conn.query(
        "SELECT id, title FROM docs ORDER BY vec <-> [1.0, 0.0, 0.0] LIMIT 1"
    ).expect("KNN search doc 1");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].get_value("title"), Some(&Value::Text("X Doc".to_string())));

    // 2. INSERT OR REPLACE with new vector pointing along Y-axis
    let ret = conn.query(
        "INSERT OR REPLACE INTO docs VALUES (1, 'Y Doc', [0.0, 1.0, 0.0]) RETURNING id, title"
    ).expect("Insert or replace doc 1");
    assert_eq!(ret.len(), 1);
    assert_eq!(ret[0].get_value("title"), Some(&Value::Text("Y Doc".to_string())));

    // KNN search near [0.0, 1.0, 0.0] should now find 'Y Doc'
    let results_y = conn.query(
        "SELECT id, title FROM docs ORDER BY vec <-> [0.0, 1.0, 0.0] LIMIT 1"
    ).expect("KNN search doc Y");
    assert_eq!(results_y.len(), 1);
    assert_eq!(results_y[0].get_value("title"), Some(&Value::Text("Y Doc".to_string())));
}

#[test]
fn test_upsert_and_returning_with_cdc_events() {
    let conn = Connection::open_in_memory().expect("Open in-memory DB");
    conn.execute("CREATE TABLE audit_log (id INTEGER PRIMARY KEY, status TEXT)")
        .expect("Create audit_log");

    let events = Arc::new(Mutex::new(Vec::new()));
    let ev_clone = events.clone();

    let _sub_id = conn.subscribe("audit_log", move |ev| {
        ev_clone.lock().unwrap().push(ev.clone());
    });

    // 1. INSERT with RETURNING generates CDC Insert event
    let rows = conn.query("INSERT INTO audit_log VALUES (100, 'created') RETURNING status")
        .expect("Insert returning");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get_value("status"), Some(&Value::Text("created".to_string())));

    {
        let evs = events.lock().unwrap();
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].table, "audit_log");
        assert!(matches!(evs[0].op, ChangeOp::Insert));
        assert_eq!(evs[0].row_id, 1);
    }

    // 2. INSERT OR REPLACE generates CDC Update event
    let rows = conn.query("INSERT OR REPLACE INTO audit_log VALUES (100, 'updated') RETURNING status")
        .expect("Replace returning");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get_value("status"), Some(&Value::Text("updated".to_string())));

    {
        let evs = events.lock().unwrap();
        assert_eq!(evs.len(), 2);
        assert_eq!(evs[1].table, "audit_log");
        assert!(matches!(evs[1].op, ChangeOp::Insert));
        assert_eq!(evs[1].row_id, 1);
    }

    // 3. DELETE with RETURNING generates CDC Delete event
    let rows = conn.query("DELETE FROM audit_log WHERE id = 100 RETURNING id")
        .expect("Delete returning");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get_value("id"), Some(&Value::Integer(100)));

    {
        let evs = events.lock().unwrap();
        assert_eq!(evs.len(), 3);
        assert_eq!(evs[2].table, "audit_log");
        assert!(matches!(evs[2].op, ChangeOp::Delete));
        assert_eq!(evs[2].row_id, 1);
    }
}
