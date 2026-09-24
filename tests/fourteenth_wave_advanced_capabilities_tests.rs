//! Fourteenth Wave Advanced Capabilities Tests for TapirusDB
//!
//! Validates:
//! 1. 16-Lane SIMD auto-vectorized kernels (Cosine, Euclidean, Dot Product) across varied dimensions.
//! 2. RaBitQ Ultra-Low Bit Random Rotation Quantization (1-Bit and 2-Bit) with single-cycle POPCNT.
//! 3. High-throughput asymmetric distance calculation and up to 32x memory compression.
//! 4. End-to-end workspace search scoring and vector embedding integration.

use tapirus::vector::{
    cosine_distance, cosine_similarity, dot_product, euclidean_distance,
    simd_cosine_distance, simd_dot_product, simd_euclidean_distance, simd_euclidean_distance_squared,
    RaBitQuantizer,
};
use tapirus::memory::embedder::{DeterministicHashEmbedder, EmbeddingEngine};

// Helper reference calculations in f64 to guarantee precision baseline
fn naive_dot_product(a: &[f32], b: &[f32]) -> f64 {
    a.iter().zip(b.iter()).map(|(&x, &y)| (x as f64) * (y as f64)).sum()
}

fn naive_euclidean_distance(a: &[f32], b: &[f32]) -> f64 {
    let sum_sq: f64 = a
        .iter()
        .zip(b.iter())
        .map(|(&x, &y)| {
            let diff = (x as f64) - (y as f64);
            diff * diff
        })
        .sum();
    sum_sq.sqrt()
}

fn naive_cosine_distance(a: &[f32], b: &[f32]) -> f64 {
    let mut dot = 0.0f64;
    let mut norm_a = 0.0f64;
    let mut norm_b = 0.0f64;
    for (&x, &y) in a.iter().zip(b.iter()) {
        let xf = x as f64;
        let yf = y as f64;
        dot += xf * yf;
        norm_a += xf * xf;
        norm_b += yf * yf;
    }
    let denom = (norm_a * norm_b).sqrt();
    if denom <= 1e-9 {
        1.0
    } else {
        1.0 - (dot / denom)
    }
}

// =========================================================================
// SECTION 1: 16-Lane SIMD Vector Acceleration Kernels
// =========================================================================

#[test]
fn test_simd_16lane_dot_product_parity() {
    let dimensions = [1, 7, 15, 16, 17, 32, 63, 64, 128, 384, 512, 768, 1536];

    for &dim in &dimensions {
        let a: Vec<f32> = (0..dim).map(|i| ((i as f32) * 0.13).sin()).collect();
        let b: Vec<f32> = (0..dim).map(|i| ((i as f32) * 0.27).cos()).collect();

        let naive = naive_dot_product(&a, &b) as f32;
        let simd = simd_dot_product(&a, &b);
        let top_level = dot_product(&a, &b);

        assert_eq!(simd, top_level, "Top-level dot_product must match SIMD kernel");
        let rel_err = (simd - naive).abs() / (naive.abs().max(1e-4));
        assert!(
            rel_err < 1e-4,
            "Dot product mismatch for dim {}: simd={}, naive={}, rel_err={}",
            dim, simd, naive, rel_err
        );
    }
}

#[test]
fn test_simd_16lane_euclidean_parity() {
    let dimensions = [1, 5, 16, 23, 64, 128, 256, 768, 1536];

    for &dim in &dimensions {
        let a: Vec<f32> = (0..dim).map(|i| (i as f32) * 0.05 + 1.0).collect();
        let b: Vec<f32> = (0..dim).map(|i| (i as f32) * 0.03 + 1.2).collect();

        let naive = naive_euclidean_distance(&a, &b) as f32;
        let simd = simd_euclidean_distance(&a, &b);
        let simd_sq = simd_euclidean_distance_squared(&a, &b);
        let top_level = euclidean_distance(&a, &b);

        assert_eq!(simd, top_level, "Top-level euclidean must match SIMD kernel");
        let rel_sq_diff = (simd * simd - simd_sq).abs() / simd_sq.max(1.0);
        assert!(rel_sq_diff < 1e-5, "Relative sq diff too high: {}", rel_sq_diff);

        let rel_diff = (simd - naive).abs() / naive.max(1.0);
        assert!(
            rel_diff < 1e-4,
            "Euclidean mismatch for dim {}: simd={}, naive={}, rel_diff={}",
            dim, simd, naive, rel_diff
        );
    }
}

#[test]
fn test_simd_16lane_cosine_parity() {
    let dimensions = [3, 16, 17, 32, 64, 128, 384, 768, 1536];

    for &dim in &dimensions {
        let a: Vec<f32> = (0..dim).map(|i| ((i as f32) * 0.1).sin() + 0.5).collect();
        let b: Vec<f32> = (0..dim).map(|i| ((i as f32) * 0.2).cos() + 0.3).collect();

        let naive = naive_cosine_distance(&a, &b) as f32;
        let simd = simd_cosine_distance(&a, &b);
        let top_level = cosine_distance(&a, &b);

        assert_eq!(simd, top_level, "Top-level cosine must match SIMD kernel");
        let diff = (simd - naive).abs();
        assert!(
            diff < 1e-4,
            "Cosine mismatch for dim {}: simd={}, naive={}, diff={}",
            dim, simd, naive, diff
        );
    }
}

#[test]
fn test_simd_vector_edge_cases() {
    let empty: [f32; 0] = [];
    assert_eq!(simd_dot_product(&empty, &empty), 0.0);
    assert_eq!(simd_euclidean_distance(&empty, &empty), 0.0);
    assert_eq!(simd_cosine_distance(&empty, &empty), 1.0);

    let zeros = vec![0.0f32; 64];
    let ones = vec![1.0f32; 64];
    assert_eq!(simd_dot_product(&zeros, &ones), 0.0);
    assert_eq!(simd_cosine_distance(&zeros, &ones), 1.0);

    // Identical vector cosine distance must be 0.0 and similarity 1.0
    let v = vec![1.2f32, -3.4, 5.6, 7.8, -9.0, 2.1, 4.3, -6.5];
    let dist = cosine_distance(&v, &v);
    let sim = cosine_similarity(&v, &v);
    assert!(dist.abs() < 1e-5);
    assert!((sim - 1.0).abs() < 1e-5);
}

// =========================================================================
// SECTION 2: RaBitQ Random Rotation Quantization (1-Bit & 2-Bit)
// =========================================================================

#[test]
fn test_rabitq_32x_memory_compression_ratio() {
    // Standard modern text embedding dimension (1536-dim)
    let dim = 1536;
    let quantizer_1bit = RaBitQuantizer::new(dim, 1);
    let vec: Vec<f32> = (0..dim).map(|i| (i as f32 * 0.01).sin()).collect();

    let q1 = quantizer_1bit.quantize(&vec);
    assert_eq!(q1.dimensions(), 1536);
    // 1536 bits = 24 u64 words. 24 * 8 bytes = 192 bytes. Plus 4 bytes norm = 196 bytes.
    // Original FP32 = 1536 * 4 = 6144 bytes.
    // 6144 / 196 = ~31.35x compression ratio!
    assert_eq!(q1.bits.len(), 24);
    assert!(q1.bits_secondary.is_none());
    assert!(
        q1.compression_ratio() >= 30.0,
        "RaBitQ 1-bit must achieve >= 30x compression factor (actual: {})",
        q1.compression_ratio()
    );

    // 2-bit quantizer
    let quantizer_2bit = RaBitQuantizer::new(dim, 2);
    let q2 = quantizer_2bit.quantize(&vec);
    assert!(q2.bits_secondary.is_some());
    assert_eq!(q2.bits_secondary.as_ref().unwrap().len(), 24);
    assert!(
        q2.compression_ratio() >= 15.0,
        "RaBitQ 2-bit must achieve >= 15x compression factor (actual: {})",
        q2.compression_ratio()
    );
}

#[test]
fn test_rabitq_hamming_distance_popcount() {
    let dim = 256;
    let quantizer = RaBitQuantizer::new(dim, 1);

    let base: Vec<f32> = (0..dim).map(|i| (i as f32 * 0.05).sin()).collect();
    let identical = base.clone();
    let opposite: Vec<f32> = base.iter().map(|&x| -x).collect();
    let orthogonal: Vec<f32> = (0..dim).map(|i| (i as f32 * 0.05).cos()).collect();

    let q_base = quantizer.quantize(&base);
    let q_ident = quantizer.quantize(&identical);
    let q_opp = quantizer.quantize(&opposite);
    let q_orth = quantizer.quantize(&orthogonal);

    // Identical vectors must have Hamming distance = 0
    assert_eq!(q_base.hamming_distance(&q_ident), 0);

    // Opposing vectors must have max Hamming distance (~256 bits flipped)
    let opp_hamming = q_base.hamming_distance(&q_opp);
    assert!(
        opp_hamming > 240,
        "Opposing vectors should have ~256 flipped bits, got {}",
        opp_hamming
    );

    // Orthogonal vectors should have ~50% bit agreement (~128 bits)
    let orth_hamming = q_base.hamming_distance(&q_orth);
    assert!(
        orth_hamming > 80 && orth_hamming < 180,
        "Orthogonal vectors should have ~50% hamming distance, got {}",
        orth_hamming
    );
}

#[test]
fn test_rabitq_asymmetric_cosine_relative_ranking() {
    let dim = 128;
    let quantizer = RaBitQuantizer::new(dim, 1);

    let query: Vec<f32> = (0..dim).map(|i| (i as f32 * 0.1).sin()).collect();
    let query_rotated = quantizer.rotate(&query);
    let query_norm: f32 = query.iter().map(|&x| x * x).sum::<f32>().sqrt();

    // Generate targets: highly correlated vs noisy vs opposite
    let mut close = query.clone();
    for i in 0..10 {
        close[i] += 0.05;
    }
    let noisy: Vec<f32> = (0..dim).map(|i| (i as f32 * 0.77).sin()).collect();
    let far: Vec<f32> = query.iter().map(|&x| -x).collect();

    let q_close = quantizer.quantize(&close);
    let q_noisy = quantizer.quantize(&noisy);
    let q_far = quantizer.quantize(&far);

    let dist_close = q_close.asymmetric_cosine_distance(&query_rotated, query_norm);
    let dist_noisy = q_noisy.asymmetric_cosine_distance(&query_rotated, query_norm);
    let dist_far = q_far.asymmetric_cosine_distance(&query_rotated, query_norm);

    assert!(
        dist_close < dist_noisy,
        "Close vector dist ({}) should be smaller than noisy dist ({})",
        dist_close, dist_noisy
    );
    assert!(
        dist_noisy < dist_far,
        "Noisy vector dist ({}) should be smaller than far dist ({})",
        dist_noisy, dist_far
    );
}

#[test]
fn test_rabitq_serialization_roundtrip() {
    let quantizer = RaBitQuantizer::with_seed(64, 2, 0xcafe1234);
    let sample: Vec<f32> = (0..64).map(|i| i as f32 * 0.25).collect();
    let quantized = quantizer.quantize(&sample);

    let serialized_q = serde_json::to_string(&quantizer).expect("Serialization failed");
    let deserialized_q: RaBitQuantizer = serde_json::from_str(&serialized_q).expect("Deserialization failed");
    assert_eq!(deserialized_q.dimensions, 64);
    assert_eq!(deserialized_q.seed, 0xcafe1234);

    let serialized_vec = serde_json::to_string(&quantized).expect("Vector serialization failed");
    let deserialized_vec: tapirus::vector::RaBitQuantizedVector =
        serde_json::from_str(&serialized_vec).expect("Vector deserialization failed");
    assert_eq!(deserialized_vec, quantized);
}

// =========================================================================
// SECTION 3: Workspace Semantic Search Engine Integration
// =========================================================================

#[test]
fn test_workspace_embedder_semantic_scoring() {
    let embedder = DeterministicHashEmbedder::new(128);

    let code_snippet = "pub fn execute_transaction(&mut self, wal: &mut WriteAheadLog) -> Result<()>";
    let query_tx = "transaction commit wal";
    let query_unrelated = "banana strawberry fruit smoothie";

    let code_vec = embedder.embed_text(code_snippet);
    let tx_vec = embedder.embed_text(query_tx);
    let unrelated_vec = embedder.embed_text(query_unrelated);

    let sim_tx = cosine_similarity(&code_vec, &tx_vec);
    let sim_unrelated = cosine_similarity(&code_vec, &unrelated_vec);

    assert!(
        sim_tx > sim_unrelated,
        "Transaction query similarity ({}) must be higher than unrelated ({})",
        sim_tx, sim_unrelated
    );
}
