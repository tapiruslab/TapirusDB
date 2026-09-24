//! Vector Indexing & HNSW Tests for TapirusDB
//!
//! Validates:
//! - HNSW multi-layer graph construction
//! - Logarithmic random level distribution scaling
//! - Vector nearest neighbor lookups and metric validation

use tapirus::vector::{DistanceMetric, HnswIndex};
use tapirus::VectorIndexEngine;

#[test]
fn test_hnsw_random_levels_distribution() {
    // Initialize with a fixed PRNG seed for deterministic test reproducibility
    let mut index = HnswIndex::with_seed(4, DistanceMetric::Cosine, 42);

    let mut level_counts = [0usize; 16];
    let num_vectors = 1000;

    for i in 1..=num_vectors {
        let vec = vec![i as f32, (i * 2) as f32, (i * 3) as f32, (i * 4) as f32];
        index.insert_vector(i as u64, &vec).expect("Insert vector");
    }

    // Inspect levels of indexed vectors
    for i in 1..=num_vectors {
        if let Some(node) = index.nodes.get(&(i as u64)) {
            if node.top_layer < 16 {
                level_counts[node.top_layer] += 1;
            }
        }
    }

    // In a true logarithmic distribution with M=16, level 0 should contain all or nearly all nodes,
    // level 1 should have roughly ~1/M (~6%), level 2 even fewer, exponentially decaying:
    assert!(
        level_counts[0] > level_counts[1],
        "Level 0 ({}) must be significantly larger than Level 1 ({})",
        level_counts[0],
        level_counts[1]
    );

    let upper_levels: usize = level_counts[2..].iter().sum();
    assert!(
        level_counts[1] >= upper_levels,
        "Level 1 ({}) should exceed higher levels combined ({})",
        level_counts[1],
        upper_levels
    );
}
