//! Third-Wave Forensic Hardening Integration Tests for TapirusDB
//!
//! Validates:
//! 1. Deadlock Immunity: Canonical Pager -> Executor lock ordering across relational & graph operations
//! 2. Secondary Index Maintenance on SQL `UPDATE`
//! 3. Full `DROP INDEX [IF EXISTS]` DDL Lifecycle
//! 4. Cryptographically Strong Multi-Entropy Salt Generation
//! 5. Document Collection `update_by_id`
//! 6. HNSW Metric Consistency in `search_knn` and bottom layer searches
//! 7. Knowledge Graph `graph_remove_node` & `graph_remove_edge` with cascade cleanup
//! 8. ANSI SQL Three-Valued Logic for `NOT IN` with NULLs

use serde_json::json;
use std::sync::Arc;
use std::thread;
use tapirus::traits::Value;
use tapirus::vector::DistanceMetric;
use tapirus::{Connection, Direction};

#[test]
fn test_secondary_index_maintenance_on_update() {
    let conn = Connection::open_in_memory().expect("Open in-memory db");

    conn.execute("CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT, status TEXT);")
        .expect("Create table");
    conn.execute("INSERT INTO users VALUES (1, 'old@tapirus.db', 'active');")
        .expect("Insert initial row");
    conn.execute("CREATE INDEX idx_users_email ON users (email);")
        .expect("Create index on email");

    // Verify initial lookup via index
    let rows = conn
        .query("SELECT id, email FROM users WHERE email = 'old@tapirus.db';")
        .expect("Query old email");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get_value("id").unwrap(), &Value::Integer(1));

    // Update email column
    let updated = conn
        .execute("UPDATE users SET email = 'new@tapirus.db' WHERE id = 1;")
        .expect("Update email");
    assert_eq!(updated, 1);

    // Old email should return 0 rows
    let rows_old = conn
        .query("SELECT id, email FROM users WHERE email = 'old@tapirus.db';")
        .expect("Query old email after update");
    assert_eq!(rows_old.len(), 0, "Old email should no longer match index");

    // New email should return 1 row
    let rows_new = conn
        .query("SELECT id, email FROM users WHERE email = 'new@tapirus.db';")
        .expect("Query new email after update");
    assert_eq!(rows_new.len(), 1, "New email must match updated index");
    assert_eq!(rows_new[0].get_value("id").unwrap(), &Value::Integer(1));
}

#[test]
fn test_drop_index_lifecycle() {
    let conn = Connection::open_in_memory().expect("Open in-memory db");

    conn.execute("CREATE TABLE products (id INTEGER PRIMARY KEY, sku TEXT, price REAL);")
        .expect("Create table");
    conn.execute("INSERT INTO products VALUES (1, 'SKU-001', 99.9);")
        .expect("Insert product");

    conn.execute("CREATE INDEX idx_products_sku ON products (sku);")
        .expect("Create index");

    // Verify index is active in explain plan
    let plan = conn
        .query("EXPLAIN SELECT id FROM products WHERE sku = 'SKU-001';")
        .expect("Explain query");
    let detail = format!("{:?}", plan[0].get_value("detail").unwrap());
    assert!(
        detail.contains("USING INDEX idx_products_sku"),
        "Explain plan should show secondary index usage"
    );

    // Drop index
    let dropped = conn
        .execute("DROP INDEX idx_products_sku;")
        .expect("Drop index");
    assert_eq!(dropped, 1);

    // Verify explain plan reverts to SCAN TABLE
    let plan_after = conn
        .query("EXPLAIN SELECT id FROM products WHERE sku = 'SKU-001';")
        .expect("Explain query after drop");
    let detail_after = format!("{:?}", plan_after[0].get_value("detail").unwrap());
    assert!(
        detail_after.contains("SCAN TABLE products"),
        "Explain plan should revert to SCAN TABLE after index drop"
    );

    // DROP INDEX IF EXISTS should succeed silently for non-existent index
    let dropped_if_exists = conn
        .execute("DROP INDEX IF EXISTS idx_products_sku;")
        .expect("Drop index if exists");
    assert_eq!(dropped_if_exists, 0);

    // DROP INDEX without IF EXISTS should error for non-existent index
    let err = conn.execute("DROP INDEX idx_nonexistent;");
    assert!(err.is_err(), "Dropping non-existent index should error");
}

#[test]
fn test_strong_salt_entropy_generation() {
    let s1 = tapirus::crypto::generate_random_salt();
    let s2 = tapirus::crypto::generate_random_salt();

    assert_eq!(s1.len(), 16);
    assert_eq!(s2.len(), 16);
    assert_ne!(s1, [0u8; 16], "Salt must not be all zeros");
    assert_ne!(s2, [0u8; 16], "Salt must not be all zeros");
    assert_ne!(s1, s2, "Consecutive salts must be cryptographically distinct");
}

#[test]
fn test_document_collection_update_by_id() {
    let conn = Connection::open_in_memory().expect("Open in-memory db");
    let coll = conn.collection("settings").expect("Open collection");

    let doc = json!({
        "theme": "dark",
        "notifications": true,
        "version": 1
    });

    let id = coll.insert_one(&doc).expect("Insert document");
    assert_eq!(coll.count().expect("Count"), 1);

    let updated_doc = json!({
        "theme": "nord",
        "notifications": false,
        "version": 2
    });

    let ok = coll.update_by_id(id, &updated_doc).expect("Update by id");
    assert!(ok, "Update should return true for existing doc");

    let retrieved = coll.find_by_id(id).expect("Find by id").expect("Doc exists");
    assert_eq!(retrieved["theme"], "nord");
    assert_eq!(retrieved["notifications"], false);
    assert_eq!(retrieved["version"], 2);

    // Total count remains 1
    assert_eq!(coll.count().expect("Count"), 1);

    // Updating non-existent doc returns false
    let missing_ok = coll.update_by_id(99999, &updated_doc).expect("Update non-existent");
    assert!(!missing_ok);
}

#[test]
fn test_hnsw_metric_override_consistency() {
    use tapirus::vector::HnswIndex;
    use tapirus::traits::VectorIndexEngine;

    let mut index = HnswIndex::new(3, DistanceMetric::Cosine);

    let v1 = vec![1.0, 0.0, 0.0];
    let v2 = vec![0.0, 1.0, 0.0];
    let v3 = vec![0.0, 0.0, 1.0];
    let v4 = vec![0.9, 0.1, 0.0];

    index.insert_vector(1, &v1).expect("Insert v1");
    index.insert_vector(2, &v2).expect("Insert v2");
    index.insert_vector(3, &v3).expect("Insert v3");
    index.insert_vector(4, &v4).expect("Insert v4");

    // Search with Euclidean metric override
    let results_euclid = index
        .search_knn(&[0.95, 0.05, 0.0], 2, DistanceMetric::Euclidean)
        .expect("Search with Euclidean override");

    assert_eq!(results_euclid.len(), 2);
    // Nearest should be v1 or v4
    let ids: Vec<u64> = results_euclid.iter().map(|(id, _)| *id).collect();
    assert!(ids.contains(&1));
    assert!(ids.contains(&4));
}

#[test]
fn test_graph_remove_node_and_edge_cleanup() {
    let conn = Connection::open_in_memory().expect("Open in-memory db");

    conn.graph_add_node(1, "Service", &json!({"name": "AuthService"}).to_string())
        .expect("Add node 1");
    conn.graph_add_node(2, "Database", &json!({"name": "UserDB"}).to_string())
        .expect("Add node 2");
    conn.graph_add_node(3, "Cache", &json!({"name": "Redis"}).to_string())
        .expect("Add node 3");

    let _e1 = conn
        .graph_add_edge(1, 2, "CONNECTS_TO", 1.0, "{}".into())
        .expect("Add edge 1->2");
    let e2 = conn
        .graph_add_edge(1, 3, "CACHES_IN", 1.0, "{}".into())
        .expect("Add edge 1->3");

    let neighbors_1 = conn.graph_neighbors(1, Direction::Outgoing, None);
    assert_eq!(neighbors_1.len(), 2);

    // Remove single edge 1->3
    let edge_removed = conn.graph_remove_edge(e2).expect("Remove edge e2");
    assert!(edge_removed);

    let neighbors_1_after_edge_drop = conn.graph_neighbors(1, Direction::Outgoing, None);
    assert_eq!(neighbors_1_after_edge_drop.len(), 1);
    assert_eq!(neighbors_1_after_edge_drop[0].0.id, 2);

    // Remove node 2, which should cascade remove edge e1
    let node_removed = conn.graph_remove_node(2).expect("Remove node 2");
    assert!(node_removed);

    let neighbors_1_final = conn.graph_neighbors(1, Direction::Outgoing, None);
    assert_eq!(neighbors_1_final.len(), 0, "Node 1 should have no remaining neighbors");

    // Removing non-existent node returns false
    let fake_remove = conn.graph_remove_node(9999).expect("Remove non-existent node");
    assert!(!fake_remove);
}

#[test]
fn test_ansi_sql_null_not_in_three_valued_logic() {
    let conn = Connection::open_in_memory().expect("Open in-memory db");

    conn.execute("CREATE TABLE items (id INTEGER PRIMARY KEY, category TEXT);")
        .expect("Create table");

    conn.execute("INSERT INTO items VALUES (1, 'electronics');")
        .expect("Insert row 1");
    conn.execute("INSERT INTO items VALUES (2, 'clothing');")
        .expect("Insert row 2");
    conn.execute("INSERT INTO items VALUES (3, NULL);")
        .expect("Insert row 3 (NULL)");

    // 1. In standard SQL: NULL NOT IN ('electronics') is UNKNOWN -> falsy for WHERE
    // Only row 2 ('clothing') should be returned!
    let rows = conn
        .query("SELECT id FROM items WHERE category NOT IN ('electronics');")
        .expect("Query NOT IN");

    assert_eq!(rows.len(), 1, "Only non-null matching row should be returned");
    assert_eq!(rows[0].get_value("id").unwrap(), &Value::Integer(2));

    // 2. NULL IN ('clothing') is UNKNOWN -> falsy for WHERE
    let rows_in = conn
        .query("SELECT id FROM items WHERE category IN ('clothing');")
        .expect("Query IN");
    assert_eq!(rows_in.len(), 1);
    assert_eq!(rows_in[0].get_value("id").unwrap(), &Value::Integer(2));
}

#[test]
fn test_abba_deadlock_immunity_concurrent_relational_and_graph() {
    let conn = Arc::new(Connection::open_in_memory().expect("Open shared in-memory db"));

    conn.execute("CREATE TABLE counter (id INTEGER PRIMARY KEY, val INTEGER);")
        .expect("Create counter table");
    conn.execute("INSERT INTO counter VALUES (1, 0);")
        .expect("Insert counter row");

    let mut handles = Vec::new();

    // Spawn 4 threads executing relational updates
    for t_id in 0..4 {
        let c = Arc::clone(&conn);
        handles.push(thread::spawn(move || {
            for i in 0..25 {
                let _ = c.execute(&format!("UPDATE counter SET val = {} WHERE id = 1;", t_id * 100 + i));
                let _ = c.query("SELECT val FROM counter WHERE id = 1;");
            }
        }));
    }

    // Spawn 4 threads executing graph mutations
    for t_id in 0..4 {
        let c = Arc::clone(&conn);
        handles.push(thread::spawn(move || {
            for i in 0..25 {
                let node_id = (t_id + 1) * 1000 + i;
                let props = format!("{{\"thread\": {t_id}}}");
                let _ = c.graph_add_node(node_id, "Worker", &props);
                if i > 0 {
                    let prev_id = node_id - 1;
                    let _ = c.graph_add_edge(prev_id, node_id, "NEXT", 1.0, "{}".into());
                }
                let _ = c.graph_neighbors(node_id, Direction::Outgoing, None);
                if i % 5 == 0 {
                    let _ = c.graph_remove_node(node_id);
                }
            }
        }));
    }

    // Join all threads - if any deadlock occurs, this would hang indefinitely
    for h in handles {
        h.join().expect("Thread completed successfully without deadlock");
    }
}
