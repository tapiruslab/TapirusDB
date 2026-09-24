//! End-to-End Integration Tests for TapirusDB.
//!
//! Validates single-file persistence, relational SQL, native AI vector search,
//! and the GraphRAG tri-model workflow.

use tapirus::{Connection, Direction, Result};
use tempfile::NamedTempFile;

#[test]
fn test_single_file_persistence_and_reopen() -> Result<()> {
    let temp_file = NamedTempFile::new().expect("Create temp file");
    let path = temp_file.path().to_path_buf();

    // 1. First Session: Create table, insert records, close connection
    {
        let db = Connection::open(&path)?;

        db.execute(
            "CREATE TABLE items (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                score REAL
            );",
        )?;

        db.execute("INSERT INTO items (id, name, score) VALUES (1, 'Alpha', 99.5);")?;
        db.execute("INSERT INTO items (id, name, score) VALUES (2, 'Beta', 88.0);")?;

        let rows = db.query("SELECT id, name, score FROM items WHERE id = 1;")?;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].get::<String>("name")?, "Alpha");
        assert_eq!(rows[0].get::<i64>("id")?, 1);
    } // db is dropped and file flushed here

    // 2. Second Session: Reopen same file from disk and verify persistence
    {
        let db = Connection::open(&path)?;

        let rows = db.query("SELECT id, name, score FROM items;")?;
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].get::<String>("name")?, "Alpha");
        assert_eq!(rows[1].get::<String>("name")?, "Beta");

        let beta_row = db.query("SELECT score FROM items WHERE id = 2;")?;
        assert_eq!(beta_row.len(), 1);
        let score: f64 = beta_row[0].get("score")?;
        assert!((score - 88.0).abs() < 1e-4);
    }

    Ok(())
}

#[test]
fn test_tri_model_graphrag_workflow() -> Result<()> {
    // Disk persistence test demonstrating Tri-Model synergy (SQL + Vector + Graph) across restart
    let temp_dir = std::env::temp_dir();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let db_path = temp_dir.join(format!("tapirus_trimodel_{nanos}.tapir"));

    {
        // 1. Open on real disk file
        let db = Connection::open(&db_path)?;

        // Relational SQL & Vector Storage
        db.execute(
            "CREATE TABLE documents (
                id INTEGER PRIMARY KEY,
                title TEXT NOT NULL,
                category TEXT,
                embedding VECTOR(4)
            );",
        )?;

        // Document 1: Machine Learning
        db.execute(
            "INSERT INTO documents (id, title, category, embedding) 
             VALUES (101, 'Transformers and Attention', 'AI Research', [0.9, 0.1, 0.0, 0.0]);",
        )?;

        // Document 2: Safe Rust
        db.execute(
            "INSERT INTO documents (id, title, category, embedding) 
             VALUES (102, 'Memory Safety in Rust 2026', 'Systems', [0.0, 0.9, 0.1, 0.0]);",
        )?;

        // Document 3: TapirusDB Engine
        db.execute(
            "INSERT INTO documents (id, title, category, embedding) 
             VALUES (103, 'TapirusDB Blueprint and Architecture', 'Databases', [0.1, 0.8, 0.8, 0.0]);",
        )?;

        // Knowledge Graph: Link entities and relationships
        db.graph_add_node(101, "Paper", r#"{"author":"Vaswani et al."}"#)?;
        db.graph_add_node(102, "Paper", r#"{"topic":"Borrow Checker"}"#)?;
        db.graph_add_node(103, "System", r#"{"author":"Ahmad Faiz"}"#)?;
        db.graph_add_node(200, "Person", r#"{"name":"Faiz"}"#)?;
        db.graph_add_node(300, "Organization", r#"{"name":"Tapirus Tech Lab"}"#)?;

        // Create knowledge edges
        db.graph_add_edge(200, 103, "ARCHITECT_OF", 1.0, "")?;
        db.graph_add_edge(200, 300, "FOUNDER_OF", 1.0, "")?;
        db.graph_add_edge(103, 102, "DEPENDS_ON_PRINCIPLE", 1.0, "")?;

        // Run ANALYZE to compute CBO statistics
        let analyzed = db.execute("ANALYZE documents;")?;
        assert_eq!(analyzed, 1, "Should analyze 1 table");

        // Verify CBO EXPLAIN outputs cost metrics
        let explain_rows = db.query("EXPLAIN QUERY PLAN SELECT * FROM documents WHERE id = 103;")?;
        assert!(!explain_rows.is_empty());
        let detail: String = explain_rows[0].get("detail")?;
        assert!(detail.contains("cost="), "EXPLAIN detail must contain CBO cost estimate: {detail}");

        db.checkpoint()?;
        // db is dropped here, closing file handles
    }

    {
        // 2. Reopen database from disk file (Simulating full restart)
        let db = Connection::open(&db_path)?;

        // Step 1 of GraphRAG: Semantic Vector Search on REOPENED disk file
        let vector_matches = db.query(
            "SELECT id, title FROM documents 
             VECTOR NEAR embedding = [0.05, 0.85, 0.2, 0.0] TOP 1;",
        )?;

        assert_eq!(vector_matches.len(), 1);
        let matched_doc_id: i64 = vector_matches[0].get("id")?;
        assert_eq!(matched_doc_id, 102, "Should match Rust Memory Safety document from disk");

        // Step 2 of GraphRAG: Knowledge Graph Traversal on REOPENED disk file
        let incoming_deps = db.graph_neighbors(102, Direction::Incoming, Some("DEPENDS_ON_PRINCIPLE"));
        assert_eq!(incoming_deps.len(), 1);
        assert_eq!(incoming_deps[0].0.id, 103, "TapirusDB depends on Rust safety");

        // Step 3 of GraphRAG: SQL Point Lookup on REOPENED disk file
        let system_rows = db.query("SELECT title, category FROM documents WHERE id = 103;")?;
        assert_eq!(system_rows.len(), 1);
        let title: String = system_rows[0].get("title")?;
        assert_eq!(title, "TapirusDB Blueprint and Architecture");

        // Subgraph Context Extraction on REOPENED disk file
        let (nodes, edges) = db.graph_subgraph(200, 2);
        assert!(nodes.len() >= 3);
        assert!(edges.len() >= 2);
    }

    let _ = std::fs::remove_file(&db_path);
    let _ = std::fs::remove_file(db_path.with_extension("tapir-wal"));
    Ok(())
}
