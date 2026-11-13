use std::collections::HashSet;
use tapirus::{
    Connection, Error, MemoryConfig, MemoryDeleteScope, MemoryDocument, MemoryFusion,
    MemoryRecallFilter, MemorySearch,
};

fn deterministic_unit_vec(seed: u64, dims: usize) -> Vec<f32> {
    let mut state = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
    let mut vec = Vec::with_capacity(dims);
    let mut norm_sq = 0.0f32;
    for _ in 0..dims {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        let unit = ((state >> 32) as f32 / (u32::MAX as f32)) * 2.0 - 1.0;
        norm_sq += unit * unit;
        vec.push(unit);
    }
    let norm = norm_sq.sqrt().max(1e-9);
    for x in &mut vec {
        *x /= norm;
    }
    vec
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na > 1e-9 && nb > 1e-9 {
        dot / (na * nb)
    } else {
        0.0
    }
}

#[test]
fn scoped_memory_enforces_hard_owner_boundary_across_all_apis() -> tapirus::Result<()> {
    let db = Connection::open_in_memory()?;
    let alice = db.scoped_memory("alice_ws", None)?;
    let alice_chat1 = alice.scoped_memory("alice_ws", Some("chat_1"))?;
    let alice_chat2 = alice.scoped_memory("alice_ws", Some("chat_2"))?;
    let bob = db.scoped_memory("bob_ws", None)?;

    // Scoped handles cannot escalate across namespaces or drop session binding
    assert!(matches!(
        alice.scoped_memory("bob_ws", None),
        Err(Error::PermissionDenied(_))
    ));
    assert!(matches!(
        alice_chat1.scoped_memory("alice_ws", Some("chat_2")),
        Err(Error::PermissionDenied(_))
    ));
    assert!(matches!(
        alice_chat1.scoped_memory("alice_ws", None),
        Err(Error::PermissionDenied(_))
    ));
    assert!(matches!(
        alice.configure_memory(MemoryConfig::default()),
        Err(Error::PermissionDenied(_))
    ));

    // Unscoped writes on scoped handles automatically inherit owner namespace/session
    let a1 = alice_chat1.store_memory(&MemoryDocument {
        content: "Alice chat 1 API token policy".into(),
        embedding: Some(vec![1.0, 0.0, 0.0, 0.0]),
        external_id: Some("doc-1".into()),
        tags: vec!["security".into()],
        ..Default::default()
    })?;
    let a2 = alice_chat2.store_memory(&MemoryDocument {
        content: "Alice chat 2 note".into(),
        embedding: Some(vec![0.0, 1.0, 0.0, 0.0]),
        ..Default::default()
    })?;
    let b1 = bob.memory_remember_text("Bob private salary spreadsheet", 0.9, &["hr"])?;

    // Cross-namespace explicit write rejected even on upsert/store_batch/JSON gateway
    assert!(matches!(
        alice.store_memory(&MemoryDocument {
            content: "Cross tenant inject".into(),
            namespace: Some("bob_ws".into()),
            ..Default::default()
        }),
        Err(Error::PermissionDenied(_))
    ));
    assert!(matches!(
        alice.memory_request_json("delete", &serde_json::json!({ "all": true }).to_string()),
        Err(Error::PermissionDenied(_))
    ));
    assert!(matches!(
        alice.delete_memories(&MemoryDeleteScope {
            all: true,
            ..Default::default()
        }),
        Err(Error::PermissionDenied(_))
    ));
    assert!(matches!(
        alice.delete_memories(&MemoryDeleteScope {
            namespace: Some("bob_ws".into()),
            ..Default::default()
        }),
        Err(Error::PermissionDenied(_))
    ));

    // Count & Get are isolated per handle
    assert_eq!(db.memory_count(), 3);
    assert_eq!(alice.memory_count(), 2);
    assert_eq!(alice_chat1.memory_count(), 1);
    assert_eq!(bob.memory_count(), 1);
    assert!(alice.memory_get(b1).is_none());
    assert!(alice_chat1.memory_get(a2).is_none());
    assert!(alice_chat1.memory_get(a1).is_some());

    // Cross-owner search, legacy recall, RRF, link, forget all blocked or empty
    assert!(matches!(
        alice.search_memories(&MemorySearch {
            query: Some("Bob".into()),
            namespace: Some("bob_ws".into()),
            ..Default::default()
        }),
        Err(Error::PermissionDenied(_))
    ));
    assert_eq!(
        alice
            .search_memories(&MemorySearch {
                query: Some("salary".into()),
                ..Default::default()
            })?
            .len(),
        0
    );
    assert!(alice.recall("salary", 5).is_empty());
    assert!(
        alice
            .memory_recall(
                Some("salary"),
                None,
                5,
                &MemoryRecallFilter {
                    namespace: Some("bob_ws".into()),
                    ..Default::default()
                }
            )
            .is_empty()
    );
    assert!(alice.memory_recall_rrf("salary", None, 5, 20.0).is_empty());

    assert!(matches!(
        alice.memory_link(a1, b1),
        Err(Error::PermissionDenied(_))
    ));
    assert!(matches!(
        alice.memory_forget_checked(b1),
        Err(Error::PermissionDenied(_))
    ));
    assert!(!alice.memory_forget(b1));
    assert!(bob.memory_get(b1).is_some());

    // Scoped prune cannot evict Bob's low-importance memory
    let bob_low = bob.memory_remember_scoped("Old temp", None, 0.01, &[], None, None)?;
    let pruned_by_alice = alice.memory_prune_at_checked(0.95, 10, u64::MAX / 4)?;
    assert!(bob.memory_get(bob_low).is_some());
    assert!(pruned_by_alice <= 2);

    Ok(())
}

#[test]
fn default_config_and_search_achieve_high_recall_at_10_on_high_dim_corpus() -> tapirus::Result<()> {
    let db = Connection::open_in_memory()?;
    let dims = 32usize;
    let total_docs = 800usize;
    let now_ts = 1_700_000_000u64;

    let mut vectors = Vec::with_capacity(total_docs);
    let mut docs = Vec::with_capacity(total_docs);
    for i in 0..total_docs {
        let v = deterministic_unit_vec(i as u64 + 100, dims);
        vectors.push(v.clone());
        docs.push(MemoryDocument {
            content: format!("Document record number {i}"),
            embedding: Some(v),
            external_id: Some(format!("vec-{i}")),
            embedding_model: Some("test-64d".into()),
            ..Default::default()
        });
    }
    let ids = db.upsert_memory_batch(&docs)?;
    db.prepare_memory_indexes()?;

    let num_queries = 20usize;
    let mut total_recall_at_10 = 0.0f64;
    for q_idx in 0..num_queries {
        let q = deterministic_unit_vec(900_000 + q_idx as u64, dims);

        let mut truth: Vec<(u64, f32)> = ids
            .iter()
            .zip(vectors.iter())
            .map(|(&id, v)| (id, cosine(&q, v)))
            .collect();
        truth.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        let truth_top10: HashSet<u64> = truth.iter().take(10).map(|(id, _)| *id).collect();

        let search = MemorySearch {
            embedding: Some(q),
            embedding_model: Some("test-64d".into()),
            limit: 10,
            ..Default::default()
        };
        let res = db.search_memories_at(&search, now_ts)?;
        let got: HashSet<u64> = res.results.iter().map(|r| r.entry.id).collect();
        let hits = truth_top10.intersection(&got).count();
        total_recall_at_10 += (hits as f64) / 10.0;
    }

    let mean_recall_at_10 = total_recall_at_10 / (num_queries as f64);
    assert!(
        mean_recall_at_10 >= 0.90,
        "Default MemoryConfig+MemorySearch exact top-10 recall={mean_recall_at_10:.3} fell below 0.90"
    );
    Ok(())
}

#[test]
fn default_hybrid_rrf_avoids_paraphrase_recall_collapse_below_vector_baseline()
-> tapirus::Result<()> {
    let db = Connection::open_in_memory()?;
    let dims = 16usize;
    let now_ts = 1_700_000_000u64;

    // 10 topics: each topic has 1 semantic paraphrase document (NO surface keyword overlap with query)
    // plus 2 surface-keyword distractor documents (exact BM25 keyword overlap, orthogonal vector).
    let mut query_vecs = Vec::new();
    let mut target_ids = Vec::new();
    let mut batch = Vec::new();

    for topic in 0..10usize {
        let mut basis = vec![0.0f32; dims];
        basis[topic] = 1.0;
        query_vecs.push((format!("topic{topic} urgent request"), basis.clone()));

        // True semantic paraphrase (different words, identical vector cone)
        let mut doc_vec = basis.clone();
        doc_vec[15] = 0.05;
        let norm: f32 = doc_vec.iter().map(|x| x * x).sum::<f32>().sqrt();
        for x in &mut doc_vec {
            *x /= norm;
        }
        batch.push(MemoryDocument {
            content: format!("paraphrased semantic solution number {topic} with zero shared terms"),
            embedding: Some(doc_vec),
            external_id: Some(format!("target-{topic}")),
            embedding_model: Some("paraphrase-16d".into()),
            importance: 0.5,
            ..Default::default()
        });

        // Two noisy keyword distractors matching query keywords in BM25 top ranks
        for d in 0..2usize {
            let mut distractor_vec = vec![0.0f32; dims];
            distractor_vec[10 + (topic + d) % 5] = 1.0;
            batch.push(MemoryDocument {
                content: format!(
                    "topic{topic} urgent request spam distractor {d} repeated topic{topic} urgent"
                ),
                embedding: Some(distractor_vec),
                external_id: Some(format!("distractor-{topic}-{d}")),
                embedding_model: Some("paraphrase-16d".into()),
                importance: 0.9,
                ..Default::default()
            });
        }
    }

    let inserted = db.upsert_memory_batch(&batch)?;
    for topic in 0..10usize {
        target_ids.push(inserted[topic * 3]);
    }
    db.prepare_memory_indexes()?;

    let mut vector_only_hits = 0usize;
    let mut default_hybrid_hits = 0usize;

    for (topic, (q_text, q_vec)) in query_vecs.iter().enumerate() {
        let target_id = target_ids[topic];

        let vec_res = db.search_memories_at(
            &MemorySearch {
                query: None,
                embedding: Some(q_vec.clone()),
                embedding_model: Some("paraphrase-16d".into()),
                limit: 5,
                ..Default::default()
            },
            now_ts,
        )?;
        if vec_res.results.iter().any(|r| r.entry.id == target_id) {
            vector_only_hits += 1;
        }

        // Default hybrid search (now tuned: rrf_k=20.0, vector_weight=0.65, lexical_weight=0.25)
        let hyb_res = db.search_memories_at(
            &MemorySearch {
                query: Some(q_text.clone()),
                embedding: Some(q_vec.clone()),
                embedding_model: Some("paraphrase-16d".into()),
                limit: 5,
                fusion: MemoryFusion::Rrf,
                ..Default::default()
            },
            now_ts,
        )?;
        assert_eq!(
            hyb_res.results[0].entry.id, target_id,
            "Paraphrase topic {topic} top-1 should beat noisy lexical distractor under default RRF"
        );
        if hyb_res.results.iter().any(|r| r.entry.id == target_id) {
            default_hybrid_hits += 1;
        }
    }

    assert_eq!(vector_only_hits, 10);
    assert_eq!(default_hybrid_hits, 10);
    Ok(())
}
