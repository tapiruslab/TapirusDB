//! Integration Tests for Wave 13 — Industrial Supremacy
//!
//! Validates the 7 strategic enhancement pillars:
//! 1. Subqueries & CTEs (`WITH ... AS (...) SELECT ...`)
//! 2. Transparent Page Compression (Safe Rust LZ4)
//! 3. Disk-Backed Paged HNSW Storage Primitives
//! 4. SQL Native Graph Predicate Filtering (`WHERE target.attr > val`)
//! 5. Micro-Columnar Vectorized Aggregation Engine
//! 6. SQLite C ABI Drop-in Compatibility Layer
//! 7. Standalone Desktop Packaging Configurations

use std::path::Path;
use tapirus::sql::vectorized::VectorizedAccumulator;
use tapirus::vector::paged_hnsw::PagedHnswIndex;
use tapirus::vector::DistanceMetric;
use tapirus::traits::Value;
use tapirus::Connection;

#[test]
fn test_pillar1_subqueries_and_ctes() {
    let conn = Connection::open_in_memory().expect("Open in memory");

    conn.execute("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, age INTEGER);")
        .unwrap();
    conn.execute("INSERT INTO users VALUES (1, 'Faiz', 30);").unwrap();
    conn.execute("INSERT INTO users VALUES (2, 'Ahmad', 22);").unwrap();
    conn.execute("INSERT INTO users VALUES (3, 'Sara', 28);").unwrap();

    conn.execute("CREATE TABLE orders (id INTEGER PRIMARY KEY, user_id INTEGER, amount REAL);")
        .unwrap();
    conn.execute("INSERT INTO orders VALUES (101, 1, 150.0);").unwrap();
    conn.execute("INSERT INTO orders VALUES (102, 3, 220.5);").unwrap();

    // 1. Basic Single CTE
    let cte_rows = conn
        .query("WITH adults AS (SELECT id, name, age FROM users WHERE age >= 25) SELECT name, age FROM adults ORDER BY age DESC;")
        .unwrap();
    assert_eq!(cte_rows.len(), 2);
    assert_eq!(cte_rows[0].get::<String>("name").unwrap(), "Faiz");
    assert_eq!(cte_rows[1].get::<String>("name").unwrap(), "Sara");

    // 2. CTE with explicit column aliases
    let cte_alias_rows = conn
        .query("WITH adult_names(user_name, user_age) AS (SELECT name, age FROM users WHERE age >= 25) SELECT user_name FROM adult_names;")
        .unwrap();
    assert_eq!(cte_alias_rows.len(), 2);

    // 3. Subquery IN
    let sub_rows = conn
        .query("SELECT name FROM users WHERE id IN (SELECT user_id FROM orders);")
        .unwrap();
    assert_eq!(sub_rows.len(), 2);
    let names: Vec<String> = sub_rows.into_iter().map(|r| r.get::<String>("name").unwrap()).collect();
    assert!(names.contains(&"Faiz".to_string()));
    assert!(names.contains(&"Sara".to_string()));
}

#[test]
fn test_pillar2_transparent_page_compression() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("compressed.tapir");

    {
        let conn = Connection::open(&db_path).expect("Create db");
        conn.enable_compression();
        assert!(conn.is_compressed(), "Compression flag should be active");

        conn.execute("CREATE TABLE docs (id INTEGER PRIMARY KEY, payload TEXT);")
            .unwrap();

        // Insert repetitive text that compresses significantly
        let sample = "{\"company\": \"Tapirus Tech Lab\", \"product\": \"TapirusDB Industrial Supremacy\", \"status\": \"verified\"} ";
        let big_payload = sample.repeat(10);

        for i in 1..=50 {
            conn.execute(&format!("INSERT INTO docs VALUES ({i}, '{big_payload}');"))
                .unwrap();
        }

        conn.checkpoint().unwrap();
    }

    // Re-open and verify transparent decompression
    {
        let conn = Connection::open(&db_path).expect("Reopen db");
        assert!(conn.is_compressed(), "Persisted compression flag should remain active");

        let rows = conn.query("SELECT COUNT(*), MAX(id) FROM docs;").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].get::<i64>("COUNT(*)").unwrap(), 50);
        assert_eq!(rows[0].get::<i64>("MAX(id)").unwrap(), 50);

        let row1 = conn.query("SELECT payload FROM docs WHERE id = 1;").unwrap();
        assert!(!row1.is_empty());
        assert!(row1[0].get::<String>("payload").unwrap().contains("Tapirus Tech Lab"));
    }
}

#[test]
fn test_pillar3_disk_backed_paged_hnsw() {
    // Test memory-bounded Paged HNSW with cache capacity smaller than total vectors
    let mut index = PagedHnswIndex::new(3, DistanceMetric::Cosine, 8);

    // Insert 25 vectors (will trigger LRU cache evictions and disk writes)
    for i in 1..=25 {
        let f = i as f32;
        index.insert(i, vec![f, f * 0.5, 1.0]).unwrap();
    }

    assert_eq!(index.store.len(), 25);

    // Search nearest neighbors
    let results = index.search(&[1.0, 0.5, 1.0], 3).unwrap();
    assert_eq!(results.len(), 3);
    assert_eq!(results[0].0, 1, "Vector 1 must be closest to query [1.0, 0.5, 1.0]");
}

#[test]
fn test_pillar4_sql_graph_predicates() {
    let conn = Connection::open_in_memory().unwrap();

    conn.execute("GRAPH INSERT NODE 1 LABEL 'Person' PROPERTIES '{\"name\": \"Alice\", \"age\": 32, \"dept\": \"Engineering\"}';")
        .unwrap();
    conn.execute("GRAPH INSERT NODE 2 LABEL 'Person' PROPERTIES '{\"name\": \"Bob\", \"age\": 22, \"dept\": \"Engineering\"}';")
        .unwrap();
    conn.execute("GRAPH INSERT NODE 3 LABEL 'Person' PROPERTIES '{\"name\": \"Charlie\", \"age\": 29, \"dept\": \"Sales\"}';")
        .unwrap();

    conn.execute("GRAPH INSERT EDGE 1 -> 2 LABEL 'COLLABORATES' WEIGHT 0.9 PROPERTIES '{}';")
        .unwrap();
    conn.execute("GRAPH INSERT EDGE 1 -> 3 LABEL 'COLLABORATES' WEIGHT 0.4 PROPERTIES '{}';")
        .unwrap();

    // 1. Traverse filtering on target node property age > 25
    let rows_age = conn
        .query("GRAPH TRAVERSE FROM 1 OUTGOING LABEL 'COLLABORATES' MAX_DEPTH 1 WHERE target.age > 25;")
        .unwrap();
    // Node 1 is root, Node 3 has age 29, Node 2 has age 22 (filtered out)
    let ids: Vec<i64> = rows_age.into_iter().map(|r| r.get::<i64>("node_id").unwrap()).collect();
    assert!(ids.contains(&3));
    assert!(!ids.contains(&2));

    // 2. Traverse filtering on edge weight >= 0.5
    let rows_weight = conn
        .query("GRAPH TRAVERSE FROM 1 OUTGOING MAX_DEPTH 1 WHERE edge.weight >= 0.5;")
        .unwrap();
    let weight_ids: Vec<i64> = rows_weight.into_iter().map(|r| r.get::<i64>("node_id").unwrap()).collect();
    assert!(weight_ids.contains(&2)); // weight 0.9
    assert!(!weight_ids.contains(&3)); // weight 0.4
}

#[test]
fn test_pillar5_vectorized_aggregation_engine() {
    let conn = Connection::open_in_memory().unwrap();

    conn.execute("CREATE TABLE metrics (val INTEGER, score REAL);").unwrap();

    for i in 1..=1000 {
        conn.execute(&format!("INSERT INTO metrics VALUES ({i}, {});", i as f64 * 1.5))
            .unwrap();
    }

    let rows = conn
        .query("SELECT COUNT(*), SUM(val), AVG(val), MIN(val), MAX(val), SUM(score) FROM metrics;")
        .unwrap();

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get::<i64>("COUNT(*)").unwrap(), 1000);
    assert_eq!(rows[0].get::<i64>("SUM(val)").unwrap(), 500500);
    assert!((rows[0].get::<f64>("AVG(val)").unwrap() - 500.5).abs() < 1e-4);
    assert_eq!(rows[0].get::<i64>("MIN(val)").unwrap(), 1);
    assert_eq!(rows[0].get::<i64>("MAX(val)").unwrap(), 1000);
    assert!((rows[0].get::<f64>("SUM(score)").unwrap() - 750750.0).abs() < 1e-3);

    // Direct unit test on VectorizedAccumulator unrolled chunk loop
    let mut acc = VectorizedAccumulator::new();
    let nums: Vec<i64> = (1..=2048).collect();
    acc.accumulate_integers(&nums, None);
    assert_eq!(acc.finalize_count(true), Value::Integer(2048));
    assert_eq!(acc.finalize_sum(), Value::Integer(2048 * 2049 / 2));
    assert_eq!(acc.finalize_min(), Value::Integer(1));
    assert_eq!(acc.finalize_max(), Value::Integer(2048));
}

#[test]
fn test_pillar7_tauri_desktop_configuration() {
    let tauri_conf_path = Path::new("studio/src-tauri/tauri.conf.json");
    assert!(tauri_conf_path.exists(), "tauri.conf.json must exist");

    let content = std::fs::read_to_string(tauri_conf_path).expect("Read tauri.conf.json");
    let json: serde_json::Value = serde_json::from_str(&content).expect("Valid JSON format");

    assert_eq!(json["productName"], "Tapirus Studio");
    assert_eq!(json["identifier"], "com.tapirusdb.studio");
    assert!(
        json["build"]["frontendDist"] == "../dist" || json["build"]["frontendDist"] == "../",
        "frontendDist should be ../dist or ../"
    );
    assert!(json["app"]["windows"][0]["title"].as_str().unwrap().contains("Tapirus Studio"));
}
