//! # Graph-to-Vector & Vector-to-Graph Chaining Integration Tests
//!
//! Validates fluent neighborhood traversal, filtered vector ranking,
//! multi-hop expansion, and cycle-safety.

use tapirus::vector::DistanceMetric;
use tapirus::{Connection, Direction, Result};
use tempfile::NamedTempFile;

#[test]
fn test_graph_to_vector_chaining_basic() -> Result<()> {
    let conn = Connection::open_in_memory()?;

    // 1. Setup Knowledge Graph with entity embeddings
    // Node 1: Patient
    conn.graph_add_node(1, "Patient", r#"{"name":"Alice"}"#)?;

    // Node 2: Aspirin (Headache treatment)
    conn.graph_add_node_with_vector(
        2,
        "Medicine",
        r#"{"name":"Aspirin","category":"analgesic"}"#,
        Some(&[0.95, 0.05, 0.0, 0.0]),
    )?;

    // Node 3: Paracetamol (Fever treatment)
    conn.graph_add_node_with_vector(
        3,
        "Medicine",
        r#"{"name":"Paracetamol","category":"antipyretic"}"#,
        Some(&[0.05, 0.95, 0.0, 0.0]),
    )?;

    // Node 4: Penicillin (Antibiotic - unrelated to symptom)
    conn.graph_add_node_with_vector(
        4,
        "Medicine",
        r#"{"name":"Penicillin","category":"antibiotic"}"#,
        Some(&[0.0, 0.0, 0.95, 0.05]),
    )?;

    // Add relationships
    conn.graph_add_edge(1, 2, "TREATS", 1.0, "")?;
    conn.graph_add_edge(1, 3, "TREATS", 0.9, "")?;
    conn.graph_add_edge(1, 4, "INCOMPATIBLE", 0.1, "")?;

    // 2. Query: From Patient #1, follow 'TREATS' edges, rank by similarity to headache symptom [0.90, 0.10, 0.0, 0.0]
    let query_symptom = [0.90, 0.10, 0.0, 0.0];
    let matches = conn
        .chain(1)
        .out(Some("TREATS"))
        .filter_label("Medicine")
        .vector_near(&query_symptom, 2, DistanceMetric::Cosine)?;

    assert_eq!(matches.len(), 2, "Should return Aspirin and Paracetamol");
    // Aspirin should be ranked #1 with high cosine similarity
    assert_eq!(matches[0].node.id, 2);
    assert!(matches[0].score > 0.98);
    // Paracetamol ranked #2
    assert_eq!(matches[1].node.id, 3);
    // Penicillin must not be present (incompatible edge label)
    assert!(matches.iter().all(|m| m.node.id != 4));

    Ok(())
}

#[test]
fn test_vector_to_graph_chaining() -> Result<()> {
    let conn = Connection::open_in_memory()?;

    // Seed entities with embeddings
    conn.graph_add_node_with_vector(
        10,
        "Paper",
        r#"{"title":"Safe Rust Database Engine"}"#,
        Some(&[0.8, 0.2, 0.0, 0.0]),
    )?;
    conn.graph_add_node_with_vector(
        20,
        "Author",
        r#"{"name":"Ahmad Faiz"}"#,
        Some(&[0.0, 0.8, 0.2, 0.0]),
    )?;
    conn.graph_add_node_with_vector(
        30,
        "Institution",
        r#"{"name":"Tapirus Tech Lab"}"#,
        None,
    )?;

    conn.graph_add_edge(10, 20, "AUTHORED_BY", 1.0, "")?;
    conn.graph_add_edge(20, 30, "AFFILIATED_WITH", 1.0, "")?;

    // Query: Seed with vector closest to Paper #10, then traverse graph to discover Author and Institution
    let query_vec = [0.82, 0.18, 0.0, 0.0];
    let chain = conn.chain_from_vector(&query_vec, 1)?;

    let authors = chain.out(Some("AUTHORED_BY")).collect_nodes();
    assert_eq!(authors.len(), 1);
    assert_eq!(authors[0].id, 20);
    assert_eq!(authors[0].label, "Author");

    Ok(())
}

#[test]
fn test_multi_hop_filtered_chain() -> Result<()> {
    let conn = Connection::open_in_memory()?;

    conn.graph_add_node(1, "A", "{}")?;
    conn.graph_add_node(2, "B", "{}")?;
    conn.graph_add_node(3, "C", "{}")?;
    conn.graph_add_node_with_vector(4, "Target", r#"{"role":"specialist"}"#, Some(&[1.0, 0.0]))?;

    conn.graph_add_edge(1, 2, "STEP_1", 1.0, "")?;
    conn.graph_add_edge(2, 3, "STEP_2", 0.9, "")?;
    conn.graph_add_edge(3, 4, "STEP_3", 0.8, "")?;

    // 3-hop chained traversal
    let matches = conn
        .chain(1)
        .out(Some("STEP_1"))
        .out(Some("STEP_2"))
        .out(Some("STEP_3"))
        .filter_label("Target")
        .vector_near(&[1.0, 0.0], 1, DistanceMetric::Cosine)?;

    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].node.id, 4);
    assert_eq!(matches[0].path.len(), 3, "Path should contain all 3 edge traversals");
    assert_eq!(matches[0].path[0].label, "STEP_1");
    assert_eq!(matches[0].path[1].label, "STEP_2");
    assert_eq!(matches[0].path[2].label, "STEP_3");

    Ok(())
}

#[test]
fn test_cycle_and_empty_neighborhood_safety() -> Result<()> {
    let conn = Connection::open_in_memory()?;

    // Create a cycle: 1 -> 2 -> 1
    conn.graph_add_node(1, "LoopNode", "{}")?;
    conn.graph_add_node(2, "LoopNode", "{}")?;
    conn.graph_add_edge(1, 2, "CYCLE", 1.0, "")?;
    conn.graph_add_edge(2, 1, "CYCLE", 1.0, "")?;

    // Multi-step traversal over cycle must terminate cleanly
    let nodes = conn
        .chain(1)
        .out(Some("CYCLE"))
        .out(Some("CYCLE"))
        .collect_nodes();

    assert!(!nodes.is_empty());

    // Non-existent edge label should yield empty list without error
    let empty_matches = conn
        .chain(1)
        .out(Some("NON_EXISTENT"))
        .vector_near(&[1.0, 2.0], 5, DistanceMetric::Cosine)?;

    assert!(empty_matches.is_empty());

    Ok(())
}

#[test]
fn test_graph_bm25_and_hybrid_chaining() -> Result<()> {
    let conn = Connection::open_in_memory()?;

    // Setup knowledge graph with rich textual properties and vector embeddings
    conn.graph_add_node_with_vector(
        100,
        "Doctor",
        r#"{"name":"Dr. House"}"#,
        None,
    )?;

    conn.graph_add_node_with_vector(
        101,
        "Paper",
        r#"{"title":"Advanced Diagnostics of Neurodegenerative Disorders in Clinical Neurology"}"#,
        Some(&[0.9, 0.1, 0.0]),
    )?;

    conn.graph_add_node_with_vector(
        102,
        "Paper",
        r#"{"title":"Pediatric Cardiology and Congenital Heart Anomalies"}"#,
        Some(&[0.1, 0.9, 0.0]),
    )?;

    conn.graph_add_edge(100, 101, "RESEARCHED", 1.0, "")?;
    conn.graph_add_edge(100, 102, "RESEARCHED", 1.0, "")?;

    // 1. Pure BM25 Lexical search over candidate neighborhood
    let bm25_matches = conn
        .chain(100)
        .out(Some("RESEARCHED"))
        .bm25_search("neurology diagnostics", 2)?;

    assert_eq!(bm25_matches.len(), 1);
    assert_eq!(bm25_matches[0].node.id, 101);
    assert!(bm25_matches[0].score > 0.0);

    // 2. Hybrid Ranking (Vector + BM25) over candidate neighborhood
    let query_vec = [0.85, 0.15, 0.0];
    let hybrid_matches = conn
        .chain(100)
        .out(Some("RESEARCHED"))
        .hybrid_rank("neurology clinical", &query_vec, 2, 0.6)?;

    assert_eq!(hybrid_matches.len(), 2);
    assert_eq!(hybrid_matches[0].node.id, 101);
    assert!(hybrid_matches[0].score > hybrid_matches[1].score);

    Ok(())
}

#[test]
fn test_graph_disk_persistence() -> Result<()> {
    let temp_file = NamedTempFile::new().expect("Temp file");
    let path = temp_file.path().to_path_buf();

    // Session 1: Create knowledge graph nodes and edges
    {
        let db = Connection::open(&path)?;

        db.graph_add_node(1, "Person", r#"{"name":"Alice","dept":"AI"}"#)?;
        db.graph_add_node_with_vector(
            2,
            "Document",
            r#"{"title":"GraphRAG Paper"}"#,
            Some(&[0.1, 0.2, 0.3, 0.4]),
        )?;
        db.graph_add_node(3, "Organization", r#"{"name":"Google DeepMind"}"#)?;

        db.graph_add_edge(1, 2, "AUTHORED", 0.95, r#"{"role":"lead"}"#)?;
        db.graph_add_edge(1, 3, "WORKS_FOR", 1.0, r#"{"since":2024}"#)?;

        let (nodes, edges) = db.graph_stats();
        assert_eq!(nodes, 3);
        assert_eq!(edges, 2);

        // Verify tables list hides internal __sys_ tables
        let tables = db.tables();
        assert!(tables.iter().all(|t| !t.name.starts_with("__sys_")));
    }

    // Session 2: Reopen and verify graph is completely restored
    {
        let db = Connection::open(&path)?;

        let (nodes, edges) = db.graph_stats();
        assert_eq!(nodes, 3, "All 3 graph nodes must be restored from disk");
        assert_eq!(edges, 2, "All 2 graph edges must be restored from disk");

        let n1_neighbors = db.graph_neighbors(1, Direction::Outgoing, Some("AUTHORED"));
        assert_eq!(n1_neighbors.len(), 1);
        assert_eq!(n1_neighbors[0].0.id, 2);
        assert_eq!(n1_neighbors[0].0.label, "Document");
        assert_eq!(n1_neighbors[0].1.label, "AUTHORED");
        assert!((n1_neighbors[0].1.weight - 0.95).abs() < 1e-4);

        // Vector on node 2 preserved
        let all_nodes = db.graph_nodes();
        let doc_node = all_nodes.iter().find(|n| n.id == 2).expect("Node 2");
        assert!(doc_node.vector.is_some());
        let vec = doc_node.vector.as_ref().unwrap();
        assert_eq!(vec, &vec![0.1, 0.2, 0.3, 0.4]);

        // Path search across restored graph
        let path = db.graph_find_path(1, 3, 5);
        assert!(path.is_some());
        assert_eq!(path.unwrap().len(), 1);
    }

    Ok(())
}
