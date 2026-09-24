//! Comprehensive Unit & Integration Tests for TapirusDB AI Agent Memory Subsystem.

use tapirus::{Connection, MemoryRecallFilter, Result};
use tempfile::NamedTempFile;

#[test]
fn test_memory_remember_and_bm25_lexical_recall() {
    let conn = Connection::open_in_memory().expect("Open in-memory DB");

    let id1 = conn
        .memory_remember(
            "User prefers replies in Bahasa Melayu and lives in Kuala Lumpur",
            None,
            0.9,
            &["preference", "language"],
        )
        .expect("Remember id1");

    let id2 = conn
        .memory_remember(
            "User is architecting TapirusDB — a 100% Safe Rust AI memory database",
            None,
            0.8,
            &["project", "systems"],
        )
        .expect("Remember id2");

    assert_eq!(id1, 1);
    assert_eq!(id2, 2);
    assert_eq!(conn.memory_count(), 2);

    // Exact keyword recall via BM25
    let filter = MemoryRecallFilter::default().with_weights(0.0, 1.0, 0.0, 0.0);
    let results = conn.memory_recall(Some("Bahasa Melayu"), None, 5, &filter);

    assert!(!results.is_empty());
    assert_eq!(results[0].entry.id, id1);
    assert!(results[0].lexical_score > 0.0);
}

#[test]
fn test_memory_dense_vector_semantic_recall() {
    let conn = Connection::open_in_memory().expect("Open in-memory DB");

    let v_tech = vec![0.9, 0.1, 0.01, 0.01];
    let v_food = vec![0.01, 0.01, 0.9, 0.1];

    let id_tech = conn
        .memory_remember(
            "Discussion on low-power microprocessors and ARM architecture",
            Some(&v_tech),
            0.7,
            &["hardware"],
        )
        .expect("Remember tech");

    let id_food = conn
        .memory_remember(
            "Favorite Malaysian meal is Nasi Lemak with spicy sambal",
            Some(&v_food),
            0.6,
            &["food"],
        )
        .expect("Remember food");

    // Query with vector close to tech
    let q_tech = vec![0.85, 0.15, 0.0, 0.0];
    let filter = MemoryRecallFilter::default().with_weights(1.0, 0.0, 0.0, 0.0);
    let results = conn.memory_recall(None, Some(&q_tech), 2, &filter);

    assert_eq!(results.len(), 2);
    assert_eq!(results[0].entry.id, id_tech);
    assert!(results[0].semantic_score > 0.9);

    // Query with vector close to food
    let q_food = vec![0.0, 0.0, 0.88, 0.12];
    let results_food = conn.memory_recall(None, Some(&q_food), 2, &filter);
    assert_eq!(results_food[0].entry.id, id_food);
    assert!(results_food[0].semantic_score > 0.9);
}

#[test]
fn test_memory_hybrid_vector_and_bm25_recall() {
    let conn = Connection::open_in_memory().expect("Open in-memory DB");

    let v_eng = vec![0.8, 0.2, 0.1];
    let v_other = vec![0.1, 0.8, 0.5];

    let id1 = conn
        .memory_remember(
            "Ahmad Faiz designed TapirusDB page-level ChaCha20 encryption",
            Some(&v_eng),
            0.9,
            &["security"],
        )
        .expect("Remember 1");

    let _id2 = conn
        .memory_remember(
            "Discussion on generic database encryption systems",
            Some(&v_other),
            0.5,
            &["general"],
        )
        .expect("Remember 2");

    // Hybrid query combining exact name "Ahmad Faiz" with semantic vector
    let filter = MemoryRecallFilter::default().with_weights(0.5, 0.5, 0.0, 0.0);
    let results = conn.memory_recall(Some("Ahmad Faiz"), Some(&v_eng), 5, &filter);

    assert!(!results.is_empty());
    assert_eq!(results[0].entry.id, id1);
    assert!(results[0].combined_score > results[1].combined_score);
}

#[test]
fn test_memory_exponential_temporal_decay() {
    let conn = Connection::open_in_memory().expect("Open in-memory DB");

    let t0 = 1_700_000_000;
    let one_week = 7 * 86400;
    let t_current = t0 + one_week;

    // Memory from 1 week ago
    let id_old = conn
        .memory_remember_at(
            "User said they want to go for a run later today",
            None,
            0.5,
            &["routine"],
            t0,
        )
        .expect("Remember old");

    // Memory from today (same content / topic)
    let id_new = conn
        .memory_remember_at(
            "User said they want to go for a run later today",
            None,
            0.5,
            &["routine"],
            t_current,
        )
        .expect("Remember new");

    // Filter with 2-day half-life so 7 days exhibits notable decay
    let filter = MemoryRecallFilter::default()
        .with_weights(0.0, 0.3, 0.7, 0.0)
        .with_half_life(2 * 86400);

    let results = conn.memory_recall_at(
        Some("run today"),
        None,
        2,
        &filter,
        t_current,
    );

    assert_eq!(results.len(), 2);
    // Recent memory must rank higher than old memory due to e^-lambda*dt
    assert_eq!(results[0].entry.id, id_new);
    assert_eq!(results[1].entry.id, id_old);
    assert!(results[0].recency_score > results[1].recency_score);
}

#[test]
fn test_memory_graph_associative_expansion() {
    let conn = Connection::open_in_memory().expect("Open in-memory DB");
    let now = 1_700_000_000;

    let id1 = conn
        .memory_remember_at("TapirusDB was created by Ahmad Faiz", None, 0.9, &[], now)
        .expect("Remember 1");

    let id2 = conn
        .memory_remember_at("HelixDB uses LMDB written in C", None, 0.8, &[], now)
        .expect("Remember 2");

    conn.memory_link(id1, id2).expect("Link memories");

    // Search specifically for "Ahmad Faiz" with graph expansion enabled
    let filter = MemoryRecallFilter::default().with_graph_hops(1);
    let results = conn.memory_recall_at(Some("Ahmad Faiz"), None, 5, &filter, now);

    assert!(results.len() >= 2);
    assert_eq!(results[0].entry.id, id1);
    assert_eq!(results[1].entry.id, id2);
    assert!(results[1].is_associative);
}

#[test]
fn test_memory_prune_decayed() {
    let conn = Connection::open_in_memory().expect("Open in-memory DB");
    let t0 = 1_000_000;

    // Low importance, old memory
    let _id_old_low = conn
        .memory_remember_at("Trivial transient remark", None, 0.1, &[], t0)
        .expect("Remember low");

    // High importance, old memory (e.g. user core identity fact)
    let _id_old_high = conn
        .memory_remember_at("User name is Faiz", None, 0.95, &[], t0)
        .expect("Remember high");

    assert_eq!(conn.memory_count(), 2);

    // After 60 days
    let t_now = t0 + 60 * 86400;
    let pruned = conn.memory_prune_at(0.3, 7 * 86400, t_now);

    // Only the low importance memory should be pruned
    assert_eq!(pruned, 1);
    assert_eq!(conn.memory_count(), 1);
}

#[test]
fn test_memory_multi_agent_namespace_isolation() {
    let conn = Connection::open_in_memory().expect("Open in-memory DB");

    // Agent Alpha stores an observation in its namespace
    conn.memory_remember_scoped(
        "Agent Alpha confidential telemetry: reactor core temperature 420K",
        None,
        0.9,
        &["telemetry"],
        Some("agent_alpha"),
        Some("session_1"),
    )
    .expect("Remember Alpha");

    // Agent Beta stores an observation in its namespace
    conn.memory_remember_scoped(
        "Agent Beta confidential telemetry: solar panel alignment 98%",
        None,
        0.9,
        &["telemetry"],
        Some("agent_beta"),
        Some("session_2"),
    )
    .expect("Remember Beta");

    assert_eq!(conn.memory_count(), 2);

    // Recall scoped to Agent Alpha
    let mut filter_alpha = MemoryRecallFilter::default();
    filter_alpha.namespace = Some("agent_alpha".to_string());
    let results_alpha = conn.memory_recall(Some("telemetry"), None, 5, &filter_alpha);
    assert_eq!(results_alpha.len(), 1);
    assert_eq!(results_alpha[0].entry.namespace.as_deref(), Some("agent_alpha"));
    assert!(results_alpha[0].entry.content.contains("reactor core"));

    // Recall scoped to Agent Beta
    let mut filter_beta = MemoryRecallFilter::default();
    filter_beta.namespace = Some("agent_beta".to_string());
    let results_beta = conn.memory_recall(Some("telemetry"), None, 5, &filter_beta);
    assert_eq!(results_beta.len(), 1);
    assert_eq!(results_beta[0].entry.namespace.as_deref(), Some("agent_beta"));
    assert!(results_beta[0].entry.content.contains("solar panel"));

    // Recall without namespace returns both
    let filter_all = MemoryRecallFilter::default();
    let results_all = conn.memory_recall(Some("telemetry"), None, 5, &filter_all);
    assert_eq!(results_all.len(), 2);
}

#[test]
fn test_memory_disk_persistence() -> Result<()> {
    let temp_file = NamedTempFile::new().expect("Temp file");
    let path = temp_file.path().to_path_buf();

    let id1;
    let id2;

    // Session 1: Store memories and link them
    {
        let db = Connection::open(&path)?;

        id1 = db.memory_remember_scoped(
            "The TapirusDB architecture uses pure safe Rust and zero C dependencies.",
            Some(&[0.5, 0.5, 0.0, 0.0]),
            0.9,
            &["architecture", "rust"],
            Some("agent_engineering"),
            Some("session_42"),
        )?;

        id2 = db.memory_remember_scoped(
            "ChaCha20-Poly1305 AEAD protects all database pages at rest.",
            Some(&[0.0, 0.0, 0.8, 0.6]),
            0.8,
            &["security", "encryption"],
            Some("agent_security"),
            Some("session_42"),
        )?;

        db.memory_link(id1, id2)?;
        assert_eq!(db.memory_count(), 2);
    }

    // Session 2: Reopen and verify memories and associations are restored
    {
        let db = Connection::open(&path)?;
        assert_eq!(db.memory_count(), 2, "Both memories must be restored from disk");

        let m1 = db.memory_get(id1).expect("Memory 1 exists");
        assert_eq!(m1.namespace.as_deref(), Some("agent_engineering"));
        assert_eq!(m1.session_id.as_deref(), Some("session_42"));
        assert_eq!(m1.tags, vec!["architecture", "rust"]);
        assert_eq!(m1.associations, vec![id2]);

        // Recall via BM25 keyword matching on restored index
        let filter = MemoryRecallFilter::default();
        let recalled = db.memory_recall(Some("pure safe Rust"), None, 5, &filter);
        assert!(!recalled.is_empty(), "Lexical BM25 must recall restored memory");
        assert_eq!(recalled[0].entry.id, id1);

        // Delete memory id1 and verify persistence of deletion
        let deleted = db.memory_forget(id1);
        assert!(deleted);
        assert_eq!(db.memory_count(), 1);
    }

    // Session 3: Reopen and verify deletion was persisted
    {
        let db = Connection::open(&path)?;
        assert_eq!(db.memory_count(), 1, "Deleted memory must remain deleted");
        assert!(db.memory_get(id1).is_none());
        assert!(db.memory_get(id2).is_some());
    }

    Ok(())
}

