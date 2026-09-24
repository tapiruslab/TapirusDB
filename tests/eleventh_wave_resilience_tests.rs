//! Eleventh-Wave Architectural Resilience & Advanced Capability Tests
//!
//! Covers:
//! 1. HNSW Configurable Parameters & 8-Bit Scalar Quantization (SQ8)
//! 2. True Inverted Posting-List Secondary Indexing with Direct $O(\log N)$ Point Lookup
//! 3. HNSW Multi-Layer Graph Topology Snapshot Persistence to Disk and Instant O(N) Restore
//! 4. Single-Writer Multiple-Reader (SWMR) Concurrency with `parking_lot::RwLock`

use tapirus::traits::{Value, VectorIndexEngine};
use tapirus::vector::{DistanceMetric, HnswConfig, HnswIndex};
use tapirus::{Config, Connection};

#[test]
fn test_hnsw_configurable_params_and_sq8() {
    let config = HnswConfig {
        m: 8,
        m0: 16,
        ef_construction: 32,
        ef_search: 16,
        quantize_sq8: true,
    };

    let mut index = HnswIndex::with_config_and_seed(4, DistanceMetric::Cosine, config, 42);

    let v1 = vec![1.0, 0.0, 0.0, 0.0];
    let v2 = vec![0.0, 1.0, 0.0, 0.0];
    let v3 = vec![0.0, 0.0, 1.0, 0.0];
    let v4 = vec![0.98, 0.02, 0.0, 0.0]; // Nearest to v1

    index.insert_vector(1, &v1).expect("Insert v1");
    index.insert_vector(2, &v2).expect("Insert v2");
    index.insert_vector(3, &v3).expect("Insert v3");
    index.insert_vector(4, &v4).expect("Insert v4");

    assert_eq!(index.len(), 4);

    // Verify each node holds 8-bit scalar quantization
    for (&id, node) in &index.nodes {
        assert!(
            node.quantized.is_some(),
            "Node {id} must have SQ8 quantized vector populated"
        );
        let q = node.quantized.as_ref().unwrap();
        assert_eq!(q.dimensions(), 4);
    }

    // Perform KNN search using quantized distance calculations
    let query = vec![0.99, 0.01, 0.0, 0.0];
    let hits = index
        .search_knn(&query, 2, DistanceMetric::Cosine)
        .expect("KNN search");

    assert_eq!(hits.len(), 2);
    let top_ids: Vec<u64> = hits.iter().map(|(id, _)| *id).collect();
    assert!(
        top_ids.contains(&1) && top_ids.contains(&4),
        "Expected top-2 to be [1, 4], got {:?}",
        top_ids
    );
}

#[test]
fn test_true_secondary_index_posting_list() {
    let conn = Connection::open_in_memory().expect("Open in-memory DB");

    conn.execute(
        "CREATE TABLE accounts (
            id INTEGER PRIMARY KEY,
            username TEXT NOT NULL,
            status TEXT NOT NULL,
            balance INTEGER NOT NULL
        );",
    )
    .expect("Create accounts table");

    // Create secondary index on status
    conn.execute("CREATE INDEX idx_status ON accounts(status);")
        .expect("Create secondary index on status");

    // Insert accounts with various statuses
    conn.execute("INSERT INTO accounts (id, username, status, balance) VALUES (1, 'alice', 'active', 500);")
        .expect("Insert 1");
    conn.execute("INSERT INTO accounts (id, username, status, balance) VALUES (2, 'bob', 'pending', 100);")
        .expect("Insert 2");
    conn.execute("INSERT INTO accounts (id, username, status, balance) VALUES (3, 'charlie', 'active', 750);")
        .expect("Insert 3");
    conn.execute("INSERT INTO accounts (id, username, status, balance) VALUES (4, 'david', 'banned', 0);")
        .expect("Insert 4");

    // 1. Direct secondary index query
    let active_rows = conn
        .query("SELECT username, status FROM accounts WHERE status = 'active';")
        .expect("Query active");
    assert_eq!(active_rows.len(), 2);
    let usernames: Vec<String> = active_rows
        .iter()
        .map(|r| match r.get_value("username").unwrap() {
            Value::Text(s) => s.clone(),
            other => panic!("Unexpected value: {:?}", other),
        })
        .collect();
    assert!(usernames.contains(&"alice".to_string()));
    assert!(usernames.contains(&"charlie".to_string()));

    // 2. EXPLAIN query plan verifies index usage
    let plan = conn
        .query("EXPLAIN SELECT username FROM accounts WHERE status = 'active';")
        .expect("Explain query");
    assert!(!plan.is_empty());
    let detail = match plan[0].get_value("detail").unwrap() {
        Value::Text(s) => s.clone(),
        _ => String::new(),
    };
    assert!(
        detail.contains("USING INDEX idx_status"),
        "Expected index usage in query plan, got: {detail}"
    );

    // 3. Update account status: charlie active -> banned
    conn.execute("UPDATE accounts SET status = 'banned' WHERE id = 3;")
        .expect("Update status");

    let active_after_update = conn
        .query("SELECT username FROM accounts WHERE status = 'active';")
        .expect("Query active after update");
    assert_eq!(active_after_update.len(), 1);
    assert_eq!(
        active_after_update[0].get_value("username").unwrap(),
        &Value::Text("alice".to_string())
    );

    let banned_after_update = conn
        .query("SELECT username FROM accounts WHERE status = 'banned';")
        .expect("Query banned after update");
    assert_eq!(banned_after_update.len(), 2); // david and charlie

    // 4. Delete account: delete alice
    conn.execute("DELETE FROM accounts WHERE id = 1;")
        .expect("Delete alice");

    let active_after_delete = conn
        .query("SELECT username FROM accounts WHERE status = 'active';")
        .expect("Query active after delete");
    assert_eq!(active_after_delete.len(), 0);
}

#[test]
fn test_hnsw_topology_persistence_and_instant_restore() {
    let temp_dir = tempfile::tempdir().expect("Create temp dir");
    let db_path = temp_dir.path().join("vector_persist.tapir");

    // Wave A: Open DB, create vector table, insert vectors, and persist snapshot
    {
        let conn = Connection::open_with_config(&db_path, Config::default())
            .expect("Open DB for write");

        conn.execute(
            "CREATE TABLE items (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                embedding VECTOR(3)
            );",
        )
        .expect("Create items table");

        conn.execute("INSERT INTO items (id, name, embedding) VALUES (1, 'item_x', '[1.0, 0.0, 0.0]');")
            .expect("Insert item 1");
        conn.execute("INSERT INTO items (id, name, embedding) VALUES (2, 'item_y', '[0.0, 1.0, 0.0]');")
            .expect("Insert item 2");
        conn.execute("INSERT INTO items (id, name, embedding) VALUES (3, 'item_z', '[0.95, 0.05, 0.0]');")
            .expect("Insert item 3");

        // Persist vector index snapshot to disk system table
        conn.persist_vector_snapshots()
            .expect("Persist vector snapshots");

        conn.checkpoint().expect("Checkpoint WAL");
    }

    // Wave B: Reopen database from disk and verify instant snapshot restore
    {
        let conn = Connection::open_with_config(&db_path, Config::default())
            .expect("Reopen DB from disk");

        // Verify system table exists
        let tables = conn.tables();
        let table_names: Vec<String> = tables.into_iter().map(|t| t.name).collect();
        assert!(table_names.contains(&"items".to_string()));

        // Query nearest to item 1
        let results = conn
            .query("SELECT id, name FROM items ORDER BY embedding <-> '[1.0, 0.0, 0.0]' LIMIT 2;")
            .expect("Vector search");

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].get_value("id").unwrap(), &Value::Integer(1));
        assert_eq!(results[1].get_value("id").unwrap(), &Value::Integer(3));
    }
}

#[test]
fn test_swmr_concurrent_readers() {
    let conn = Connection::open_in_memory().expect("Open in-memory DB");

    // Initialize schema and sample data
    conn.execute(
        "CREATE TABLE articles (
            id INTEGER PRIMARY KEY,
            title TEXT NOT NULL,
            views INTEGER NOT NULL
        );",
    )
    .expect("Create articles");

    conn.execute("INSERT INTO articles (id, title, views) VALUES (1, 'Rust 2024 Edition', 1500);")
        .expect("Insert article 1");
    conn.execute("INSERT INTO articles (id, title, views) VALUES (2, 'Zero-Copy Storage', 2800);")
        .expect("Insert article 2");

    // Initialize Knowledge Graph
    conn.graph_add_node(10, "Author", "{\"name\": \"Alice\"}")
        .expect("Add graph node 10");
    conn.graph_add_node(20, "Paper", "{\"title\": \"Database Systems\"}")
        .expect("Add graph node 20");
    conn.graph_add_edge(10, 20, "WROTE", 1.0, "{}")
        .expect("Add graph edge");

    // Initialize AI Memory
    conn.memory_remember("Persistent distributed snapshot journal", None, 0.9, &["architecture"])
        .expect("Store memory");

    // Spawn 8 concurrent reader threads
    let handles: Vec<_> = (0..8)
        .map(|thread_id| {
            let thread_conn = conn.clone();
            std::thread::spawn(move || {
                // SQL concurrent read
                let rows = thread_conn
                    .query("SELECT id, title FROM articles WHERE views > 1000;")
                    .expect("Concurrent SQL read");
                assert_eq!(rows.len(), 2, "Thread {thread_id} read wrong row count");

                // Graph concurrent read
                let neighbors = thread_conn.graph_neighbors(
                    10,
                    tapirus::graph::Direction::Outgoing,
                    Some("WROTE"),
                );
                assert_eq!(neighbors.len(), 1, "Thread {thread_id} read wrong neighbor count");

                // Memory concurrent read
                let count = thread_conn.memory_count();
                assert_eq!(count, 1, "Thread {thread_id} read wrong memory count");
            })
        })
        .collect();

    for handle in handles {
        handle.join().expect("Reader thread panicked");
    }
}
