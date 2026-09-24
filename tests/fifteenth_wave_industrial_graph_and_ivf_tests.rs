//! Fifteenth Wave Industrial Graph and IVF Partitioned Index Tests for TapirusDB
//!
//! Validates:
//! 1. Compressed Sparse Row (CSR) and Column (CSC) contiguous graph topology layout.
//! 2. Zero-allocation neighbor slice queries, binary search edge existence, and WCOJ triangle counting.
//! 3. Inverted File (IVF) k-means Voronoi partitioning combined with RaBitQ 1-bit POPCNT distance.
//! 4. Declarative openCypher pattern matching runtime (`MATCH (a)-[r]->(b) WHERE ... RETURN ...`).
//! 5. Variable-length path expansion (`*1..3`) and seamless transparent routing via `conn.query()`.

use tapirus::graph::csr::CsrGraph;
use tapirus::graph::cypher::CypherExecutor;
use tapirus::graph::GraphEngine;
use tapirus::vector::ivf::{IvfConfig, IvfIndex};
use tapirus::vector::quantization::RaBitQuantizer;
use tapirus::vector::DistanceMetric;
use tapirus::Connection;

// =========================================================================
// SECTION 1: Compressed Sparse Row (CSR) Contiguous Graph Topology
// =========================================================================

#[test]
fn test_csr_topology_contiguous_slices_and_bidirectional_traversal() {
    let mut engine = GraphEngine::new();

    // Nodes 1..=5
    let _ = engine.add_node(1, "User", r#"{"name": "Alice"}"#);
    let _ = engine.add_node(2, "User", r#"{"name": "Bob"}"#);
    let _ = engine.add_node(3, "User", r#"{"name": "Charlie"}"#);
    let _ = engine.add_node(4, "User", r#"{"name": "David"}"#);
    let _ = engine.add_node(5, "User", r#"{"name": "Eve"}"#);

    // Edges: 1->2, 1->3, 2->4, 3->4, 4->5
    let _ = engine.add_edge(1, 2, "FOLLOWS", 1.0, "");
    let _ = engine.add_edge(1, 3, "FOLLOWS", 0.9, "");
    let _ = engine.add_edge(2, 4, "FOLLOWS", 0.8, "");
    let _ = engine.add_edge(3, 4, "FOLLOWS", 0.7, "");
    let _ = engine.add_edge(4, 5, "FOLLOWS", 0.6, "");

    let csr = CsrGraph::from_graph_engine(&engine);

    assert_eq!(csr.node_count(), 5);
    assert_eq!(csr.edge_count(), 5);

    // 1. Contiguous outgoing slice checks
    let out_1 = csr.outgoing_neighbors(1);
    assert_eq!(out_1, &[2, 3], "Outgoing neighbors of 1 must be contiguous slice [2, 3]");
    assert_eq!(csr.outgoing_degree(1), 2);

    let out_4 = csr.outgoing_neighbors(4);
    assert_eq!(out_4, &[5]);
    assert_eq!(csr.outgoing_degree(4), 1);

    let out_5 = csr.outgoing_neighbors(5);
    assert!(out_5.is_empty(), "Node 5 has no outgoing edges");

    // 2. Contiguous incoming slice checks (CSC)
    let in_4 = csr.incoming_neighbors(4);
    assert_eq!(in_4, &[2, 3], "Incoming neighbors of 4 must be contiguous slice [2, 3]");
    assert_eq!(csr.incoming_degree(4), 2);

    let in_1 = csr.incoming_neighbors(1);
    assert!(in_1.is_empty(), "Node 1 has no incoming edges");

    // 3. Fast binary search edge existence
    assert!(csr.has_edge(1, 2));
    assert!(csr.has_edge(1, 3));
    assert!(csr.has_edge(3, 4));
    assert!(!csr.has_edge(2, 1), "Edge is directional: 1->2, not 2->1");
    assert!(!csr.has_edge(1, 5));

    // 4. Contiguous BFS traversal
    let bfs_order = csr.bfs(1, 3);
    assert_eq!(bfs_order, vec![1, 2, 3, 4, 5]);

    // 5. Roundtrip conversion to GraphEngine
    let recovered_engine = csr.to_graph_engine();
    assert_eq!(recovered_engine.node_count(), 5);
    assert_eq!(recovered_engine.edge_count(), 5);

    // 6. Binary serialization roundtrip
    let bytes = csr.to_bytes().expect("CSR serialization failed");
    let recovered_csr = CsrGraph::from_bytes(&bytes).expect("CSR deserialization failed");
    assert_eq!(recovered_csr.node_count(), 5);
    assert_eq!(recovered_csr.outgoing_neighbors(1), &[2, 3]);
}

#[test]
fn test_csr_triangle_counting_and_clique_intersection() {
    let mut engine = GraphEngine::new();

    // Create a 4-node graph with two triangles sharing an edge: (1, 2, 3) and (2, 3, 4)
    for i in 1..=4 {
        let _ = engine.add_node(i, "Vertex", "");
    }

    // Bidirectional edges to form undirected triangles
    let edges = [
        (1, 2), (2, 1),
        (2, 3), (3, 2),
        (3, 1), (1, 3),
        (2, 4), (4, 2),
        (3, 4), (4, 3),
    ];

    for (u, v) in edges {
        let _ = engine.add_edge(u, v, "EDGE", 1.0, "");
    }

    let csr = engine.to_csr();

    // Verify common neighbors between 2 and 3 (should be 1 and 4)
    let common = csr.common_neighbors_count(2, 3);
    assert_eq!(common, 2, "Nodes 2 and 3 must share common neighbors 1 and 4");

    // Total triangles in graph should be 2
    let triangles = csr.triangle_count();
    assert_eq!(triangles, 2, "Graph must contain exactly 2 triangles");
}

// =========================================================================
// SECTION 2: Inverted File (IVF) Clustering Index Partitioning
// =========================================================================

#[test]
fn test_ivf_kmeans_clustering_and_search_recall() {
    let dim = 32;
    let n_vectors = 150;
    let mut vectors = Vec::with_capacity(n_vectors);

    for i in 0..n_vectors {
        let v: Vec<f32> = (0..dim)
            .map(|j| ((i * 17 + j) as f32 * 0.1).sin())
            .collect();
        vectors.push((i as u64, v));
    }

    let config = IvfConfig {
        dimensions: dim,
        num_clusters: 8,
        default_n_probe: 3,
        metric: DistanceMetric::Cosine,
    };

    let index = IvfIndex::train_and_build(&vectors, config, None);
    assert_eq!(index.len(), n_vectors);
    assert_eq!(index.clusters.len(), 8);

    // Query with vector 25: top-1 must be ID 25 with near-zero distance
    let results = index.search(&vectors[25].1, 5, 4);
    assert!(!results.is_empty());
    assert_eq!(results[0].0, 25);
    assert!(results[0].1 < 1e-4);

    // Dynamic insertion test after initial training
    let mut mutable_index = index;
    let new_vec: Vec<f32> = (0..dim).map(|j| (j as f32 * 0.5).cos()).collect();
    mutable_index.insert(999, &new_vec);
    assert_eq!(mutable_index.len(), n_vectors + 1);

    let new_res = mutable_index.search(&new_vec, 3, 4);
    assert_eq!(new_res[0].0, 999);
}

#[test]
fn test_ivf_rabitq_quantized_extreme_compression_search() {
    let dim = 64;
    let n_vectors = 80;
    let quantizer = RaBitQuantizer::new(dim, 1);

    let mut vectors = Vec::with_capacity(n_vectors);
    for i in 0..n_vectors {
        let v: Vec<f32> = (0..dim)
            .map(|j| ((i * 13 + j) as f32 * 0.05).sin())
            .collect();
        vectors.push((i as u64, v));
    }

    let config = IvfConfig {
        dimensions: dim,
        num_clusters: 6,
        default_n_probe: 3,
        metric: DistanceMetric::Cosine,
    };

    let index = IvfIndex::train_and_build(&vectors, config, Some(quantizer));
    assert_eq!(index.len(), n_vectors);

    // Verify clusters contain quantized RaBitQ vectors
    for cluster in &index.clusters {
        assert_eq!(cluster.vector_ids.len(), cluster.rabit_vectors.len());
        assert!(cluster.raw_vectors.is_empty(), "Raw vectors should not be kept when quantizer is active");
    }

    // Search with vector 42
    let results = index.search(&vectors[42].1, 5, 4);
    assert!(!results.is_empty());
    // Candidate 42 should rank high in top candidates
    let found_42 = results.iter().any(|r| r.0 == 42);
    assert!(found_42, "Target vector 42 must be retrieved in IVF-RaBitQ search results");
}

// =========================================================================
// SECTION 3: Declarative openCypher Pattern Matching & Integration
// =========================================================================

#[test]
fn test_opencypher_match_and_where_and_return() {
    let conn = Connection::open_in_memory().expect("Failed to open connection");

    // Setup graph
    conn.execute("GRAPH INSERT NODE 1 LABEL 'Person' PROPERTIES '{\"name\": \"Alice\", \"age\": 30}';")
        .expect("Insert Alice failed");
    conn.execute("GRAPH INSERT NODE 2 LABEL 'Person' PROPERTIES '{\"name\": \"Bob\", \"age\": 25}';")
        .expect("Insert Bob failed");
    conn.execute("GRAPH INSERT NODE 3 LABEL 'Company' PROPERTIES '{\"name\": \"TapirusTech\"}';")
        .expect("Insert Company failed");

    conn.execute("GRAPH INSERT EDGE 1 -> 2 LABEL 'KNOWS' WEIGHT 0.9;")
        .expect("Insert KNOWS failed");
    conn.execute("GRAPH INSERT EDGE 2 -> 3 LABEL 'WORKS_AT' WEIGHT 1.0;")
        .expect("Insert WORKS_AT failed");

    // 1. MATCH with label filter and projection
    let cypher = "MATCH (a:Person)-[r:KNOWS]->(b:Person) RETURN b.name, r.weight";
    let rows = conn.query_cypher(cypher).expect("Cypher execution failed");

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get::<String>("b.name").unwrap(), "Bob");
    assert!((rows[0].get::<f64>("r.weight").unwrap() - 0.9).abs() < 1e-4);

    // 2. MATCH with WHERE clause
    let cypher_where = "MATCH (a:Person)-[r]->(b) WHERE b.name = 'Bob' RETURN a.name, b.name";
    let rows_where = conn.query_cypher(cypher_where).expect("Cypher WHERE failed");
    assert_eq!(rows_where.len(), 1);
    assert_eq!(rows_where[0].get::<String>("a.name").unwrap(), "Alice");
    assert_eq!(rows_where[0].get::<String>("b.name").unwrap(), "Bob");

    // 3. Transparent execution via standard `conn.query()`
    let rows_transparent = conn.query("MATCH (a:Person)-[r:KNOWS]->(b:Person) RETURN b.name")
        .expect("Transparent query routing failed");
    assert_eq!(rows_transparent.len(), 1);
    assert_eq!(rows_transparent[0].get::<String>("b.name").unwrap(), "Bob");
}

#[test]
fn test_opencypher_multihop_path_expansion() {
    let mut engine = GraphEngine::new();

    // Linear chain: 1 -> 2 -> 3 -> 4
    for i in 1..=4 {
        let _ = engine.add_node(i, "Item", &format!(r#"{{"val": {i}}}"#));
    }
    let _ = engine.add_edge(1, 2, "NEXT", 1.0, "");
    let _ = engine.add_edge(2, 3, "NEXT", 1.0, "");
    let _ = engine.add_edge(3, 4, "NEXT", 1.0, "");

    // Variable-length traversal: 1 to 3 hops from node 1
    let query = "MATCH (a:Item)-[:NEXT*1..3]->(b:Item) WHERE a.id = 1 RETURN b.id";
    let rows = CypherExecutor::execute_query(&engine, query).expect("Multihop execution failed");

    assert_eq!(rows.len(), 3, "Should reach nodes 2, 3, and 4 within 1..3 hops");
    let target_ids: Vec<i64> = rows.iter().map(|r| r.get::<i64>("b.id").unwrap()).collect();
    assert!(target_ids.contains(&2));
    assert!(target_ids.contains(&3));
    assert!(target_ids.contains(&4));
}

#[test]
fn test_opencypher_create_statement() {
    let conn = Connection::open_in_memory().expect("Failed to open connection");

    // Execute Cypher CREATE via conn.execute
    let affected = conn.execute("CREATE (x:Developer {name: 'Faiz', specialty: 'Databases'})")
        .expect("CREATE statement failed");
    assert_eq!(affected, 1);

    // Verify node exists
    let nodes = conn.graph_nodes();
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0].label, "Developer");
    assert!(nodes[0].properties.contains("Faiz"));
}
