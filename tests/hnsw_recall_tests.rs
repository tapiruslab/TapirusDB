use tapirus::{DistanceMetric, HnswIndex, VectorIndexEngine};

fn vectors(count: usize, dimensions: usize) -> Vec<Vec<f32>> {
    let mut state = 0x7c9e3a21u32;
    (0..count)
        .map(|_| {
            (0..dimensions)
                .map(|_| {
                    state ^= state << 13;
                    state ^= state >> 17;
                    state ^= state << 5;
                    (state as f64 / u32::MAX as f64 * 2.0 - 1.0) as f32
                })
                .collect()
        })
        .collect()
}

#[test]
fn recent_vectors_remain_reachable_and_recall_matches_exact_search() {
    let data = vectors(1_000, 32);
    for seed in [42, 20_261_016] {
        let mut index = HnswIndex::with_seed(32, DistanceMetric::Cosine, seed);
        for (id, vector) in data.iter().enumerate() {
            index.insert_vector(id as u64 + 1, vector).unwrap();
        }
        assert_eq!(
            index
                .search_knn(&data[999], 1, DistanceMetric::Cosine)
                .unwrap()[0]
                .0,
            1000
        );
        let mut hits = 0;
        for query in data.iter().skip(500).step_by(10) {
            let mut exact: Vec<_> = data
                .iter()
                .enumerate()
                .map(|(id, vector)| {
                    (
                        id as u64 + 1,
                        1.0 - tapirus::memory::cosine_similarity(query, vector),
                    )
                })
                .collect();
            exact.sort_by(|a, b| a.1.total_cmp(&b.1));
            let neighbors = index.search_knn(query, 10, DistanceMetric::Cosine).unwrap();
            hits += neighbors
                .iter()
                .filter(|(id, _)| exact.iter().take(10).any(|(expected, _)| id == expected))
                .count();
        }
        let recall = hits as f64 / 500.0;
        assert!(recall >= 0.90, "recall@10={recall}, seed={seed}");
        let restored = HnswIndex::import_snapshot(index.export_snapshot());
        assert_eq!(
            restored
                .search_knn(&data[999], 1, DistanceMetric::Cosine)
                .unwrap()[0]
                .0,
            1000
        );
    }
}

#[test]
fn replacing_a_deleted_vector_revives_it_and_invalid_input_preserves_old_data() {
    let mut index = HnswIndex::with_seed(2, DistanceMetric::Cosine, 42);
    index.insert_vector(1, &[1.0, 0.0]).unwrap();
    index.mark_deleted(1);
    index.insert_vector(1, &[0.0, 1.0]).unwrap();
    assert!(!index.is_deleted(1));
    assert_eq!(index.len(), 1);
    assert!(index.insert_vector(1, &[f32::NAN, 0.0]).is_err());
    assert_eq!(
        index
            .search_knn(&[0.0, 1.0], 1, DistanceMetric::Cosine)
            .unwrap()[0]
            .0,
        1
    );
    assert!(
        index
            .search_knn(&[f32::INFINITY, 0.0], 1, DistanceMetric::Cosine)
            .is_err()
    );
}

#[test]
fn selective_filters_and_heavy_deletions_keep_surviving_vectors_searchable() {
    let data = vectors(300, 16);
    let mut index = HnswIndex::with_seed(16, DistanceMetric::Cosine, 42);
    for (id, vector) in data.iter().enumerate() {
        index.insert_vector(id as u64 + 1, vector).unwrap();
    }
    let allowed = std::collections::HashSet::from([300]);
    assert_eq!(
        index
            .search_knn_filtered(&data[0], 1, DistanceMetric::Cosine, Some(&allowed))
            .unwrap()[0]
            .0,
        300
    );
    for id in 1..=280 {
        index.mark_deleted(id);
    }
    assert_eq!(
        index
            .search_knn(&data[0], 10, DistanceMetric::Cosine)
            .unwrap()
            .len(),
        10
    );
    assert_eq!(index.rebuild().unwrap(), 280);
    assert_eq!(index.len(), 20);
    for id in 281..=300 {
        assert_eq!(
            index
                .search_knn(&data[id - 1], 1, DistanceMetric::Cosine)
                .unwrap()[0]
                .0,
            id as u64
        );
    }
}
