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
        db.graph_add_node(103, "System", r#"{"author":"Alex Chen"}"#)?;
        db.graph_add_node(200, "Person", r#"{"name":"Alex"}"#)?;
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

#[test]
fn test_tap_grounded_sql_decision_workflow() -> Result<()> {
    use std::sync::Arc;
    use tapirus::traits::VectorIndexEngine;
    use tapirus::vector::{DistanceMetric, HnswIndex};
    use tapirus::tap::register_grounding_index;

    let db = Connection::open_in_memory()?;

    // 1. Create table with audit entries
    db.execute("CREATE TABLE complaints (id INTEGER PRIMARY KEY, message TEXT, department TEXT);")?;
    db.execute("INSERT INTO complaints VALUES (1, 'Barang pecah bila sampai, mohon refund balik duit', 'support');")?;
    db.execute("INSERT INTO complaints VALUES (2, 'Password reset email was never received', 'it');")?;

    // 2. Setup HNSW grounding index
    let mut hnsw = HnswIndex::new(64, DistanceMetric::Cosine);
    let sample_vec = vec![0.05; 64];
    hnsw.insert_vector(999, &sample_vec)?;
    register_grounding_index("kb_policies", Arc::new(hnsw));

    // 3. Query using TAP_CLASSIFY_GROUNDED and TAP_VERIFY_GROUNDED in SQL
    let rows = db.query(
        "SELECT id, \
         TAP_CLASSIFY_GROUNDED(message, 'refund, login, sales', 'kb_policies', 1) AS category, \
         TAP_VERIFY_GROUNDED(message, 'barang rosak mohon refund duit', 'kb_policies', 1) AS is_refund_request \
         FROM complaints WHERE id = 1;",
    )?;

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get::<String>("category")?, "refund");
    assert_eq!(rows[0].get::<i64>("is_refund_request")?, 1);

    Ok(())
}

#[test]
fn test_online_chatbot_cognitive_flow() -> Result<()> {
    let db = Connection::open_in_memory()?;

    // 1. Initialize chatbot relational tables
    db.execute(
        "CREATE TABLE IF NOT EXISTS tap_chat_logs (
            id INTEGER PRIMARY KEY,
            session_id TEXT,
            user_message TEXT,
            bot_reply TEXT,
            intent TEXT,
            confidence REAL,
            is_safe INTEGER,
            latency_us INTEGER,
            created_at TEXT
        );",
    )?;

    db.execute(
        "CREATE TABLE IF NOT EXISTS tap_knowledge_base (
            id INTEGER PRIMARY KEY,
            category TEXT,
            keywords TEXT,
            title TEXT,
            content TEXT,
            language TEXT
        );",
    )?;

    // 2. Insert verified knowledge records
    db.execute(
        "INSERT INTO tap_knowledge_base (id, category, keywords, title, content, language) \
         VALUES (1, 'billing_and_enterprise_plans', 'pelan enterprise harga lesen', 'Pelan Enterprise TapirusDB', 'Sokongan 24/7 SLA dan replikasi multi-node', 'ms');",
    )?;

    // 3. Cognitive intent classification on Malay inquiry
    let message = "Saya nak tahu mengenai pelan enterprise TapirusDB";
    let candidate_intents = [
        "database_architecture",
        "tap_cognitive_engine",
        "billing_and_enterprise_plans",
        "general_greeting",
    ];

    let classify_res = db.tap().classify(message, &candidate_intents)?;
    assert_eq!(classify_res.top_choice, "billing_and_enterprise_plans");
    assert!(classify_res.confidence > 0.30);

    // 4. Verification check
    let verify_res = db.tap().verify("Pengguna ingin maklumat tentang pelan enterprise TapirusDB.", message)?;
    assert!(verify_res.is_verified);

    // 5. Query knowledge base
    let rows = db.query("SELECT title, content FROM tap_knowledge_base WHERE category = 'billing_and_enterprise_plans' AND language = 'ms';")?;
    assert_eq!(rows.len(), 1);
    let title = rows[0].get::<String>("title")?;
    let content = rows[0].get::<String>("content")?;
    assert_eq!(title, "Pelan Enterprise TapirusDB");

    // 6. Persist dialogue to relational SQL
    let reply = format!("**{}**\n{}", title, content);
    let insert_sql = format!(
        "INSERT INTO tap_chat_logs (id, session_id, user_message, bot_reply, intent, confidence, is_safe, latency_us, created_at) \
         VALUES (1, 'sess-test', '{}', '{}', '{}', {:.4}, 1, 850, '1760000000');",
        message, reply, classify_res.top_choice, classify_res.confidence
    );
    db.execute(&insert_sql)?;

    // 7. Verify audit log retrieval from SQL
    let logs = db.query("SELECT id, session_id, intent, is_safe FROM tap_chat_logs WHERE id = 1;")?;
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0].get::<i64>("id")?, 1);
    assert_eq!(logs[0].get::<String>("session_id")?, "sess-test");
    assert_eq!(logs[0].get::<String>("intent")?, "billing_and_enterprise_plans");
    assert_eq!(logs[0].get::<i64>("is_safe")?, 1);

    Ok(())
}

