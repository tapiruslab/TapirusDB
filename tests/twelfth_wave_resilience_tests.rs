//! Wave 12 Resilience Test Suite
//!
//! Validates:
//! 1. Hybrid Vector Search with SQL Pre-Filtering (`VECTOR NEAR ... TOP k WHERE ...`)
//! 2. Secondary Indexing on Nested JSON Paths (`CREATE INDEX ... ON table(doc.path.field)`)
//! 3. SQL Native Graph Traversal (`GRAPH TRAVERSE` and `GRAPH SHORTEST_PATH`)
//! 4. Mathematical Graph Algorithms (PageRank & Weighted Dijkstra Shortest Path)
//! 5. SQL JSON Extraction Functions (`JSON_EXTRACT(col, '$.path')` / nested dot notation)

use tapirus::Connection;
use serde_json::json;

#[test]
fn test_hybrid_vector_search_with_prefiltering() {
    let conn = Connection::open_in_memory().expect("Open in-memory DB");

    conn.execute(
        "CREATE TABLE products (id INTEGER PRIMARY KEY, name TEXT, category TEXT, price REAL, embedding VECTOR(3));"
    ).expect("Create products table");

    // Insert records with vectors and categories
    // Vector [1.0, 0.0, 0.0] -> electronics
    conn.execute(
        "INSERT INTO products (id, name, category, price, embedding) VALUES (1, 'Laptop', 'electronics', 1200.0, [0.99, 0.01, 0.0]);"
    ).expect("Insert 1");
    // Vector very close to [1.0, 0.0, 0.0], but category is 'apparel'
    conn.execute(
        "INSERT INTO products (id, name, category, price, embedding) VALUES (2, 'T-Shirt', 'apparel', 25.0, [0.98, 0.02, 0.0]);"
    ).expect("Insert 2");
    // Another 'electronics' item with slightly further vector
    conn.execute(
        "INSERT INTO products (id, name, category, price, embedding) VALUES (3, 'Smartphone', 'electronics', 800.0, [0.85, 0.15, 0.0]);"
    ).expect("Insert 3");
    // 'furniture' item
    conn.execute(
        "INSERT INTO products (id, name, category, price, embedding) VALUES (4, 'Desk Chair', 'furniture', 150.0, [0.0, 1.0, 0.0]);"
    ).expect("Insert 4");

    // Query 1: Unfiltered Vector Search - row 1 and row 2 are closest
    let rows_unfiltered = conn.query(
        "SELECT id, name, category FROM products VECTOR NEAR embedding = [1.0, 0.0, 0.0] TOP 2;"
    ).expect("Unfiltered vector search");
    assert_eq!(rows_unfiltered.len(), 2);
    let ids_unfiltered: Vec<i64> = rows_unfiltered.iter().map(|r| r.get("id").unwrap()).collect();
    assert_eq!(ids_unfiltered, vec![1, 2]);

    // Query 2: Hybrid Pre-filtered Vector Search - only 'electronics' allowed
    let rows_filtered = conn.query(
        "SELECT id, name, category FROM products VECTOR NEAR embedding = [1.0, 0.0, 0.0] TOP 2 WHERE category = 'electronics';"
    ).expect("Filtered vector search");
    assert_eq!(rows_filtered.len(), 2);
    let ids_filtered: Vec<i64> = rows_filtered.iter().map(|r| r.get("id").unwrap()).collect();
    // Row 2 ('apparel') must be filtered out, so row 1 and row 3 ('electronics') must be returned
    assert_eq!(ids_filtered, vec![1, 3]);

    for r in &rows_filtered {
        let cat: String = r.get("category").unwrap();
        assert_eq!(cat, "electronics");
    }
}

#[test]
fn test_json_path_secondary_index_and_point_lookup() {
    let conn = Connection::open_in_memory().expect("Open in-memory DB");

    conn.execute(
        "CREATE TABLE accounts (id INTEGER PRIMARY KEY, profile TEXT);"
    ).expect("Create accounts table");

    // Create a secondary B+Tree index directly on nested JSON path: profile.location.city
    conn.execute(
        "CREATE INDEX idx_account_city ON accounts(profile.location.city);"
    ).expect("Create JSON path index");

    // Insert JSON accounts
    let user1 = json!({
        "name": "Faiz",
        "location": { "city": "Cyberjaya", "country": "MY" },
        "active": true
    }).to_string();

    let user2 = json!({
        "name": "Marcus",
        "location": { "city": "Singapore", "country": "SG" },
        "active": true
    }).to_string();

    let user3 = json!({
        "name": "Ahmad",
        "location": { "city": "Cyberjaya", "country": "MY" },
        "active": false
    }).to_string();

    conn.execute(&format!("INSERT INTO accounts (id, profile) VALUES (1, '{user1}');")).expect("Insert user 1");
    conn.execute(&format!("INSERT INTO accounts (id, profile) VALUES (2, '{user2}');")).expect("Insert user 2");
    conn.execute(&format!("INSERT INTO accounts (id, profile) VALUES (3, '{user3}');")).expect("Insert user 3");

    // Query using JSON path filter: profile.location.city = 'Cyberjaya'
    let rows = conn.query(
        "SELECT id, profile.name, profile.location.city FROM accounts WHERE profile.location.city = 'Cyberjaya';"
    ).expect("Query by JSON path");

    assert_eq!(rows.len(), 2);
    let ids: Vec<i64> = rows.iter().map(|r| r.get("id").unwrap()).collect();
    assert!(ids.contains(&1) && ids.contains(&3));

    // Verify extracted field projections
    let names: Vec<String> = rows.iter().map(|r| r.get("profile.name").unwrap()).collect();
    assert!(names.contains(&"Faiz".to_string()) && names.contains(&"Ahmad".to_string()));
}

#[test]
fn test_graph_sql_traverse_and_shortest_path() {
    let conn = Connection::open_in_memory().expect("Open in-memory DB");

    // Insert Nodes
    conn.execute("GRAPH INSERT NODE 1 LABEL Person PROPERTIES '{\"name\":\"Alice\"}';").expect("Node 1");
    conn.execute("GRAPH INSERT NODE 2 LABEL Person PROPERTIES '{\"name\":\"Bob\"}';").expect("Node 2");
    conn.execute("GRAPH INSERT NODE 3 LABEL Person PROPERTIES '{\"name\":\"Charlie\"}';").expect("Node 3");
    conn.execute("GRAPH INSERT NODE 4 LABEL Person PROPERTIES '{\"name\":\"Diana\"}';").expect("Node 4");

    // Insert Edges: 1 -> 2 -> 3 -> 4
    conn.execute("GRAPH INSERT EDGE 1 2 LABEL KNOWS;").expect("Edge 1->2");
    conn.execute("GRAPH INSERT EDGE 2 3 LABEL KNOWS;").expect("Edge 2->3");
    conn.execute("GRAPH INSERT EDGE 3 4 LABEL KNOWS;").expect("Edge 3->4");

    // Test SQL GRAPH TRAVERSE from node 1 with max depth 2
    let traverse_rows = conn.query(
        "GRAPH TRAVERSE FROM 1 OUTGOING LABEL KNOWS MAX_DEPTH 2;"
    ).expect("Execute GRAPH TRAVERSE");

    assert_eq!(traverse_rows.len(), 3); // node 1 (depth 0), node 2 (depth 1), node 3 (depth 2)
    let traversed_ids: Vec<i64> = traverse_rows.iter().map(|r| r.get("node_id").unwrap()).collect();
    assert_eq!(traversed_ids, vec![1, 2, 3]);

    // Test SQL GRAPH SHORTEST_PATH from 1 to 4
    let path_rows = conn.query(
        "GRAPH SHORTEST_PATH FROM 1 TO 4;"
    ).expect("Execute GRAPH SHORTEST_PATH");

    assert_eq!(path_rows.len(), 3); // step 1 (1->2), step 2 (2->3), step 3 (3->4)
    let steps: Vec<i64> = path_rows.iter().map(|r| r.get("step").unwrap()).collect();
    assert_eq!(steps, vec![1, 2, 3]);
}

#[test]
fn test_graph_pagerank_and_dijkstra_algorithms() {
    let conn = Connection::open_in_memory().expect("Open in-memory DB");

    // Build directed graph with weighted edges
    conn.execute("GRAPH INSERT NODE 10 LABEL Page PROPERTIES 'Home';").unwrap();
    conn.execute("GRAPH INSERT NODE 20 LABEL Page PROPERTIES 'About';").unwrap();
    conn.execute("GRAPH INSERT NODE 30 LABEL Page PROPERTIES 'Docs';").unwrap();
    conn.execute("GRAPH INSERT NODE 40 LABEL Page PROPERTIES 'Blog';").unwrap();

    // 10 -> 20 (weight 2.0)
    // 20 -> 30 (weight 3.0)  => Path 10->20->30 cost = 5.0
    // 10 -> 40 (weight 10.0)
    // 40 -> 30 (weight 1.0)  => Path 10->40->30 cost = 11.0
    conn.execute("GRAPH INSERT EDGE 10 20 LABEL LINKS;").unwrap();
    conn.execute("GRAPH INSERT EDGE 20 30 LABEL LINKS;").unwrap();
    conn.execute("GRAPH INSERT EDGE 10 40 LABEL LINKS;").unwrap();
    conn.execute("GRAPH INSERT EDGE 40 30 LABEL LINKS;").unwrap();

    // Verify Dijkstra weighted shortest path
    let shortest = conn.graph_dijkstra_path(10, 30).expect("Compute dijkstra path");
    let (path_edges, total_cost) = shortest;
    assert_eq!(path_edges.len(), 2);
    assert_eq!(path_edges[0].from_id, 10);
    assert_eq!(path_edges[0].to_id, 20);
    assert_eq!(path_edges[1].from_id, 20);
    assert_eq!(path_edges[1].to_id, 30);
    assert!(total_cost > 0.0);

    // Verify PageRank computation
    let ranks = conn.graph_pagerank(0.85, 50, 0.0001);
    assert_eq!(ranks.len(), 4);
    let total_pr: f32 = ranks.values().sum();
    // Sum of PageRanks must approximate 1.0
    assert!((total_pr - 1.0).abs() < 0.01, "PageRank sum should be ~1.0, got {}", total_pr);

    // Node 30 has 2 incoming edges (from 20 and 40), so its rank must be higher than 10 (which has 0 incoming edges)
    let pr_10 = ranks.get(&10).copied().unwrap_or(0.0);
    let pr_30 = ranks.get(&30).copied().unwrap_or(0.0);
    assert!(pr_30 > pr_10, "Target node 30 should have higher rank than source node 10");
}

#[test]
fn test_json_extract_function_in_sql() {
    let conn = Connection::open_in_memory().expect("Open in-memory DB");

    conn.execute(
        "CREATE TABLE documents (id INTEGER PRIMARY KEY, content TEXT);"
    ).expect("Create documents table");

    let doc1 = json!({
        "order_id": "ORD-101",
        "amount": 250,
        "customer": { "name": "Faiz", "tier": "gold" }
    }).to_string();

    let doc2 = json!({
        "order_id": "ORD-102",
        "amount": 75,
        "customer": { "name": "Siti", "tier": "silver" }
    }).to_string();

    conn.execute(&format!("INSERT INTO documents (id, content) VALUES (1, '{doc1}');")).unwrap();
    conn.execute(&format!("INSERT INTO documents (id, content) VALUES (2, '{doc2}');")).unwrap();

    // Test JSON_EXTRACT in projection and WHERE filter
    let rows = conn.query(
        "SELECT id, JSON_EXTRACT(content, '$.order_id'), JSON_EXTRACT(content, '$.customer.tier') FROM documents WHERE JSON_EXTRACT(content, '$.amount') >= 100;"
    ).expect("Query with JSON_EXTRACT");

    assert_eq!(rows.len(), 1);
    let id: i64 = rows[0].get("id").unwrap();
    assert_eq!(id, 1);

    let order_id: String = rows[0].get("JSON_EXTRACT(content, '$.order_id')").unwrap();
    assert_eq!(order_id, "ORD-101");

    let tier: String = rows[0].get("JSON_EXTRACT(content, '$.customer.tier')").unwrap();
    assert_eq!(tier, "gold");
}
