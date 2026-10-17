use tapirus::{
    Connection, MemoryConfig, MemoryDeleteScope, MemoryDocument, MemoryEngine, MemorySearch,
};

fn source(id: &str, text: &str, vector: Vec<f32>) -> MemoryDocument {
    MemoryDocument {
        content: text.into(),
        external_id: Some(id.into()),
        embedding: Some(vector),
        embedding_model: Some("test/unit-v1".into()),
        namespace: Some("manuals".into()),
        session_id: Some("robot-7".into()),
        tags: vec!["source:manual".into()],
        ..Default::default()
    }
}
fn search(vector: Vec<f32>) -> MemorySearch {
    MemorySearch {
        embedding: Some(vector),
        embedding_model: Some("test/unit-v1".into()),
        namespace: Some("MANUALS".into()),
        session_id: Some("robot-7".into()),
        vector_weight: 1.0,
        lexical_weight: 0.0,
        recency_weight: 0.0,
        importance_weight: 0.0,
        ..Default::default()
    }
}

#[test]
fn source_upsert_revision_atomic_batch_and_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("memory.tapir");
    let db = Connection::open(&file).unwrap();
    let first = source("manual-1", "Original maintenance policy", vec![1.0, 0.0]);
    let id = db.store_memory(&first).unwrap();
    assert!(db.store_memory(&first).is_err());
    let mut next = source("manual-1", "Updated maintenance policy", vec![0.0, 1.0]);
    next.expected_revision = Some(1);
    next.source_version = Some("2026.2".into());
    assert_eq!(db.upsert_memory(&next).unwrap(), id);
    assert_eq!(db.memory_get(id).unwrap().revision, 2);
    assert_eq!(db.memory_get(id).unwrap().content_hash.len(), 64);
    let mut conflict = source("manual-2", "Must not be inserted", vec![1.0, 0.0]);
    conflict.expected_revision = Some(99);
    next.expected_revision = Some(2);
    next.content = "Must rollback".into();
    assert!(db.upsert_memory_batch(&[next, conflict]).is_err());
    assert_eq!(db.memory_count(), 1);
    assert_eq!(
        db.search_memories(&search(vec![0.0, 1.0])).unwrap()[0]
            .entry
            .content,
        "Updated maintenance policy"
    );
    db.close().unwrap();
    drop(db);
    let reopened = Connection::open(&file).unwrap();
    let result = reopened.search_memories(&search(vec![0.0, 1.0])).unwrap();
    assert_eq!(result[0].entry.id, id);
    assert_eq!(result[0].entry.revision, 2);
    assert_eq!(result[0].entry.source_version.as_deref(), Some("2026.2"));
}

#[test]
fn vector_index_freshness_savepoints_scopes_and_model_contract() {
    let db = Connection::open_in_memory().unwrap();
    let id = db
        .upsert_memory(&source("a", "Engine maintenance", vec![1.0, 0.0]))
        .unwrap();
    assert_eq!(db.prepare_memory_indexes().unwrap().indexed_vectors, 1);
    db.execute("BEGIN").unwrap();
    db.execute("SAVEPOINT change").unwrap();
    db.upsert_memory(&source("a", "Engine revised", vec![0.0, 1.0]))
        .unwrap();
    assert_eq!(
        db.search_memories(&search(vec![0.0, 1.0])).unwrap()[0]
            .entry
            .id,
        id
    );
    assert!(
        db.search_memories(&search(vec![1.0, 0.0]))
            .unwrap()
            .is_empty()
    );
    db.execute("ROLLBACK TO change").unwrap();
    db.query("COMMIT").unwrap();
    assert_eq!(
        db.search_memories(&search(vec![1.0, 0.0])).unwrap()[0]
            .entry
            .content,
        "Engine maintenance"
    );
    assert!(
        db.store_memory(&source(
            "wrong",
            "Invalid model dimensions",
            vec![1.0, 0.0, 0.0]
        ))
        .is_err()
    );
    let mut query = search(vec![1.0, 0.0]);
    query.embedding_model = None;
    assert!(
        db.search_memories(&query).unwrap().is_empty(),
        "Legacy and named model spaces must not mix"
    );
    query = search(vec![1.0, 0.0, 0.0]);
    assert!(db.search_memories(&query).is_err());
    query = search(vec![1.0, 0.0]);
    query.tags = vec!["missing".into()];
    assert!(db.search_memories(&query).unwrap().is_empty());
}

#[test]
fn expiry_scope_deletion_and_association_cleanup_are_durable() {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("expiry.tapir");
    let db = Connection::open(&file).unwrap();
    let mut expiring = source("expires", "Expired source", vec![1.0, 0.0]);
    expiring.expires_at = Some(100);
    let expired_id = db.store_memory(&expiring).unwrap();
    let id = db
        .store_memory(&source("kept", "Retained source", vec![1.0, 0.0]))
        .unwrap();
    db.memory_link(id, expired_id).unwrap();
    assert_eq!(
        db.search_memories_at(&search(vec![1.0, 0.0]), 99)
            .unwrap()
            .results
            .len(),
        2
    );
    assert_eq!(
        db.search_memories_at(&search(vec![1.0, 0.0]), 100)
            .unwrap()
            .results
            .len(),
        1
    );
    assert_eq!(db.purge_expired_memories_at(100).unwrap(), 1);
    assert!(db.memory_get(id).unwrap().associations.is_empty());
    assert!(db.delete_memories(&MemoryDeleteScope::default()).is_err());
    db.close().unwrap();
    drop(db);
    let db = Connection::open(&file).unwrap();
    assert!(db.memory_get(id).unwrap().associations.is_empty());
    assert_eq!(
        db.delete_memories(&MemoryDeleteScope {
            external_id: Some("kept".into()),
            namespace: Some("manuals".into()),
            ..Default::default()
        })
        .unwrap(),
        1
    );
    assert!(
        db.search_memories(&search(vec![1.0, 0.0]))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn resource_limits_survive_failed_batches_and_rollback() {
    let db = Connection::open_in_memory().unwrap();
    let config = MemoryConfig {
        max_documents: 2,
        max_batch_documents: 3,
        ..Default::default()
    };
    db.configure_memory(config).unwrap();
    db.store_memory(&source("one", "first", vec![1.0, 0.0]))
        .unwrap();
    assert!(
        db.store_memory_batch(&[
            source("two", "second", vec![1.0, 0.0]),
            source("three", "third", vec![1.0, 0.0])
        ])
        .is_err()
    );
    assert_eq!(db.memory_count(), 1);
    db.store_memory(&source("two", "second", vec![1.0, 0.0]))
        .unwrap();
    assert!(
        db.remember("Legacy wrapper must obey the same budget")
            .is_err()
    );
    db.execute("BEGIN").unwrap();
    db.memory_forget_checked(1).unwrap();
    db.execute("ROLLBACK").unwrap();
    assert!(
        db.store_memory(&source("three", "third", vec![1.0, 0.0]))
            .is_err()
    );
    let previous = db.memory_stats().estimated_resident_bytes;
    assert!(
        db.configure_memory(MemoryConfig {
            max_estimated_bytes: 1,
            ..Default::default()
        })
        .is_err()
    );
    assert_eq!(db.memory_stats().estimated_resident_bytes, previous);
    assert_eq!(
        db.prepare_memory_indexes().unwrap().pending_index_updates,
        0
    );
}

#[test]
fn serialized_engines_rebuild_derived_indexes_and_extreme_vectors_are_finite() {
    let mut engine = MemoryEngine::new();
    engine.remember(
        "large finite components",
        Some(&[f32::MAX, f32::MAX]),
        0.5,
        &[],
        1,
    );
    let bytes = serde_json::to_vec(&engine).unwrap();
    let mut restored: MemoryEngine = serde_json::from_slice(&bytes).unwrap();
    let response = restored
        .search_indexed(
            &MemorySearch {
                embedding: Some(vec![f32::MAX, f32::MAX]),
                ..Default::default()
            },
            2,
        )
        .unwrap();
    assert_eq!(response.results.len(), 1);
    assert!(response.results[0].semantic_score > 0.99);
    assert!(response.results[0].combined_score.is_finite());
}

#[test]
fn indexed_memory_retrieval_matches_exact_neighbors_without_scoring_the_corpus() {
    let db = Connection::open_in_memory().unwrap();
    let mut state = 20_261_016u64;
    let mut random = || {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        ((state >> 32) as u32 as f64 / u32::MAX as f64 * 2.0 - 1.0) as f32
    };
    let documents: Vec<_> = (0..1200)
        .map(|id| {
            source(
                &format!("doc-{id}"),
                &format!("robot engineering source {id}"),
                (0..24).map(|_| random()).collect(),
            )
        })
        .collect();
    db.store_memory_batch(&documents).unwrap();
    db.prepare_memory_indexes().unwrap();
    let mut hits = 0;
    for _ in 0..25 {
        let mut query = search((0..24).map(|_| random()).collect());
        query.limit = 10;
        let approximate = db.search_memories_detailed(&query).unwrap();
        query.exact = true;
        let exact = db.search_memories_detailed(&query).unwrap();
        hits += approximate
            .results
            .iter()
            .filter(|r| exact.results.iter().any(|e| e.entry.id == r.entry.id))
            .count();
        assert_eq!(approximate.diagnostics.vector_indexes_used, 1);
        assert!(!approximate.diagnostics.exact_vector_search);
        assert!(approximate.diagnostics.scored_candidates <= 128);
    }
    assert!(hits as f64 / 250.0 >= 0.90, "recall@10={} hits", hits);
}

#[test]
fn version_replacement_removes_only_stale_chunks_inside_the_requested_source() {
    let db = Connection::open_in_memory().unwrap();
    for (key, version, tag) in [
        ("a:0", "v1", "source:a"),
        ("a:1", "v1", "source:a"),
        ("b:0", "v1", "source:b"),
    ] {
        db.upsert_memory(&MemoryDocument {
            content: "source chunk".into(),
            external_id: Some(key.into()),
            source_version: Some(version.into()),
            namespace: Some("PÉRAK".into()),
            tags: vec![tag.into()],
            ..Default::default()
        })
        .unwrap();
    }
    db.execute("BEGIN").unwrap();
    db.upsert_memory(&MemoryDocument {
        content: "revised source chunk".into(),
        external_id: Some("a:0".into()),
        source_version: Some("v2".into()),
        namespace: Some("pérak".into()),
        tags: vec!["source:a".into()],
        ..Default::default()
    })
    .unwrap();
    assert_eq!(
        db.delete_memories(&MemoryDeleteScope {
            namespace: Some("pérak".into()),
            tags: vec!["source:a".into()],
            except_source_version: Some("v2".into()),
            ..Default::default()
        })
        .unwrap(),
        1
    );
    db.execute("COMMIT").unwrap();
    assert_eq!(db.memory_count(), 2);
    assert!(
        db.delete_memories(&MemoryDeleteScope {
            except_source_version: Some("v2".into()),
            ..Default::default()
        })
        .is_err()
    );
}

#[test]
fn existing_model_spaces_cannot_exceed_a_lowered_admission_budget() {
    let db = Connection::open_in_memory().unwrap();
    for model in ["model-a", "model-b"] {
        db.store_memory(&MemoryDocument {
            content: format!("source {model}"),
            embedding: Some(vec![1.0, 0.0]),
            embedding_model: Some(model.into()),
            ..Default::default()
        })
        .unwrap();
    }
    assert!(
        db.configure_memory(MemoryConfig {
            max_vector_spaces: 1,
            ..Default::default()
        })
        .is_err()
    );
    assert_eq!(db.memory_stats().vector_spaces, 2);
}
