//! High-Performance SIMD-Optimized Vector Math Kernels
//!
//! Provides 16-lane parallel loop unrolling and multi-lane accumulator pipelines
//! aligned with modern CPU instruction-level parallelism (AVX2, AVX-512, and ARM NEON)
//! under 100% Safe Rust (`#![forbid(unsafe_code)]`).

/// Calculate Cosine Distance between two floating point slices using a cascading 16-8-4-1 SIMD pipeline.
///
/// Returns `1.0 - CosineSimilarity`. If dimensions mismatch or are empty, returns `1.0`.
#[inline]
pub fn simd_cosine_distance(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 1.0;
    }

    let mut dot_0 = 0.0f32;
    let mut dot_1 = 0.0f32;
    let mut dot_2 = 0.0f32;
    let mut dot_3 = 0.0f32;

    let mut na_0 = 0.0f32;
    let mut na_1 = 0.0f32;
    let mut na_2 = 0.0f32;
    let mut na_3 = 0.0f32;

    let mut nb_0 = 0.0f32;
    let mut nb_1 = 0.0f32;
    let mut nb_2 = 0.0f32;
    let mut nb_3 = 0.0f32;

    // 1. Primary 16-lane unrolling (AVX-512 / dual AVX2 register saturation)
    let chunks_a16 = a.chunks_exact(16);
    let chunks_b16 = b.chunks_exact(16);
    let rem_a16 = chunks_a16.remainder();
    let rem_b16 = chunks_b16.remainder();

    for (ca, cb) in chunks_a16.zip(chunks_b16) {
        dot_0 += ca[0] * cb[0] + ca[1] * cb[1] + ca[2] * cb[2] + ca[3] * cb[3];
        dot_1 += ca[4] * cb[4] + ca[5] * cb[5] + ca[6] * cb[6] + ca[7] * cb[7];
        dot_2 += ca[8] * cb[8] + ca[9] * cb[9] + ca[10] * cb[10] + ca[11] * cb[11];
        dot_3 += ca[12] * cb[12] + ca[13] * cb[13] + ca[14] * cb[14] + ca[15] * cb[15];

        na_0 += ca[0] * ca[0] + ca[1] * ca[1] + ca[2] * ca[2] + ca[3] * ca[3];
        na_1 += ca[4] * ca[4] + ca[5] * ca[5] + ca[6] * ca[6] + ca[7] * ca[7];
        na_2 += ca[8] * ca[8] + ca[9] * ca[9] + ca[10] * ca[10] + ca[11] * ca[11];
        na_3 += ca[12] * ca[12] + ca[13] * ca[13] + ca[14] * ca[14] + ca[15] * ca[15];

        nb_0 += cb[0] * cb[0] + cb[1] * cb[1] + cb[2] * cb[2] + cb[3] * cb[3];
        nb_1 += cb[4] * cb[4] + cb[5] * cb[5] + cb[6] * cb[6] + cb[7] * cb[7];
        nb_2 += cb[8] * cb[8] + cb[9] * cb[9] + cb[10] * cb[10] + cb[11] * cb[11];
        nb_3 += cb[12] * cb[12] + cb[13] * cb[13] + cb[14] * cb[14] + cb[15] * cb[15];
    }

    // 2. Secondary 8-lane cascade (AVX2 256-bit register saturation)
    let chunks_a8 = rem_a16.chunks_exact(8);
    let chunks_b8 = rem_b16.chunks_exact(8);
    let rem_a8 = chunks_a8.remainder();
    let rem_b8 = chunks_b8.remainder();

    for (ca, cb) in chunks_a8.zip(chunks_b8) {
        dot_0 += ca[0] * cb[0] + ca[1] * cb[1] + ca[2] * cb[2] + ca[3] * cb[3];
        dot_1 += ca[4] * cb[4] + ca[5] * cb[5] + ca[6] * cb[6] + ca[7] * cb[7];

        na_0 += ca[0] * ca[0] + ca[1] * ca[1] + ca[2] * ca[2] + ca[3] * ca[3];
        na_1 += ca[4] * ca[4] + ca[5] * ca[5] + ca[6] * ca[6] + ca[7] * ca[7];

        nb_0 += cb[0] * cb[0] + cb[1] * cb[1] + cb[2] * cb[2] + cb[3] * cb[3];
        nb_1 += cb[4] * cb[4] + cb[5] * cb[5] + cb[6] * cb[6] + cb[7] * cb[7];
    }

    // 3. Tertiary 4-lane cascade (SSE/ARM NEON 128-bit register saturation)
    let chunks_a4 = rem_a8.chunks_exact(4);
    let chunks_b4 = rem_b8.chunks_exact(4);
    let rem_a4 = chunks_a4.remainder();
    let rem_b4 = chunks_b4.remainder();

    for (ca, cb) in chunks_a4.zip(chunks_b4) {
        dot_2 += ca[0] * cb[0] + ca[1] * cb[1] + ca[2] * cb[2] + ca[3] * cb[3];
        na_2 += ca[0] * ca[0] + ca[1] * ca[1] + ca[2] * ca[2] + ca[3] * ca[3];
        nb_2 += cb[0] * cb[0] + cb[1] * cb[1] + cb[2] * cb[2] + cb[3] * cb[3];
    }

    let mut dot = (dot_0 + dot_1) + (dot_2 + dot_3);
    let mut norm_a = (na_0 + na_1) + (na_2 + na_3);
    let mut norm_b = (nb_0 + nb_1) + (nb_2 + nb_3);

    // 4. Final 1-3 scalar tail
    for (x, y) in rem_a4.iter().zip(rem_b4.iter()) {
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }

    if norm_a <= 0.0 || norm_b <= 0.0 || !norm_a.is_finite() || !norm_b.is_finite() || !dot.is_finite() {
        return 1.0;
    }

    let denom = norm_a.sqrt() * norm_b.sqrt();
    if denom <= 0.0 || !denom.is_finite() {
        return 1.0;
    }

    let sim = (dot / denom).clamp(-1.0, 1.0);
    if !sim.is_finite() {
        return 1.0;
    }
    1.0 - sim
}

/// Calculate Squared Euclidean Distance (L2) between two floating point slices using a cascading 16-8-4-1 SIMD pipeline.
#[inline]
pub fn simd_euclidean_distance_squared(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return f32::MAX;
    }

    let mut sum_0 = 0.0f32;
    let mut sum_1 = 0.0f32;
    let mut sum_2 = 0.0f32;
    let mut sum_3 = 0.0f32;

    // 1. Primary 16-lane unrolling
    let chunks_a16 = a.chunks_exact(16);
    let chunks_b16 = b.chunks_exact(16);
    let rem_a16 = chunks_a16.remainder();
    let rem_b16 = chunks_b16.remainder();

    for (ca, cb) in chunks_a16.zip(chunks_b16) {
        let d0 = ca[0] - cb[0];
        let d1 = ca[1] - cb[1];
        let d2 = ca[2] - cb[2];
        let d3 = ca[3] - cb[3];
        sum_0 += d0 * d0 + d1 * d1 + d2 * d2 + d3 * d3;

        let d4 = ca[4] - cb[4];
        let d5 = ca[5] - cb[5];
        let d6 = ca[6] - cb[6];
        let d7 = ca[7] - cb[7];
        sum_1 += d4 * d4 + d5 * d5 + d6 * d6 + d7 * d7;

        let d8 = ca[8] - cb[8];
        let d9 = ca[9] - cb[9];
        let d10 = ca[10] - cb[10];
        let d11 = ca[11] - cb[11];
        sum_2 += d8 * d8 + d9 * d9 + d10 * d10 + d11 * d11;

        let d12 = ca[12] - cb[12];
        let d13 = ca[13] - cb[13];
        let d14 = ca[14] - cb[14];
        let d15 = ca[15] - cb[15];
        sum_3 += d12 * d12 + d13 * d13 + d14 * d14 + d15 * d15;
    }

    // 2. Secondary 8-lane cascade
    let chunks_a8 = rem_a16.chunks_exact(8);
    let chunks_b8 = rem_b16.chunks_exact(8);
    let rem_a8 = chunks_a8.remainder();
    let rem_b8 = chunks_b8.remainder();

    for (ca, cb) in chunks_a8.zip(chunks_b8) {
        let d0 = ca[0] - cb[0];
        let d1 = ca[1] - cb[1];
        let d2 = ca[2] - cb[2];
        let d3 = ca[3] - cb[3];
        sum_0 += d0 * d0 + d1 * d1 + d2 * d2 + d3 * d3;

        let d4 = ca[4] - cb[4];
        let d5 = ca[5] - cb[5];
        let d6 = ca[6] - cb[6];
        let d7 = ca[7] - cb[7];
        sum_1 += d4 * d4 + d5 * d5 + d6 * d6 + d7 * d7;
    }

    // 3. Tertiary 4-lane cascade
    let chunks_a4 = rem_a8.chunks_exact(4);
    let chunks_b4 = rem_b8.chunks_exact(4);
    let rem_a4 = chunks_a4.remainder();
    let rem_b4 = chunks_b4.remainder();

    for (ca, cb) in chunks_a4.zip(chunks_b4) {
        let d0 = ca[0] - cb[0];
        let d1 = ca[1] - cb[1];
        let d2 = ca[2] - cb[2];
        let d3 = ca[3] - cb[3];
        sum_2 += d0 * d0 + d1 * d1 + d2 * d2 + d3 * d3;
    }

    let mut total = (sum_0 + sum_1) + (sum_2 + sum_3);
    // 4. Final 1-3 scalar tail
    for (x, y) in rem_a4.iter().zip(rem_b4.iter()) {
        let diff = x - y;
        total += diff * diff;
    }

    total
}

/// Calculate Euclidean Distance (L2 norm) between two floating point slices.
#[inline]
pub fn simd_euclidean_distance(a: &[f32], b: &[f32]) -> f32 {
    let sq = simd_euclidean_distance_squared(a, b);
    if sq.is_finite() {
        sq.sqrt()
    } else {
        f32::MAX
    }
}

/// Calculate Dot Product between two floating point slices using a cascading 16-8-4-1 SIMD pipeline.
#[inline]
pub fn simd_dot_product(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }

    let mut dot_0 = 0.0f32;
    let mut dot_1 = 0.0f32;
    let mut dot_2 = 0.0f32;
    let mut dot_3 = 0.0f32;

    // 1. Primary 16-lane unrolling
    let chunks_a16 = a.chunks_exact(16);
    let chunks_b16 = b.chunks_exact(16);
    let rem_a16 = chunks_a16.remainder();
    let rem_b16 = chunks_b16.remainder();

    for (ca, cb) in chunks_a16.zip(chunks_b16) {
        dot_0 += ca[0] * cb[0] + ca[1] * cb[1] + ca[2] * cb[2] + ca[3] * cb[3];
        dot_1 += ca[4] * cb[4] + ca[5] * cb[5] + ca[6] * cb[6] + ca[7] * cb[7];
        dot_2 += ca[8] * cb[8] + ca[9] * cb[9] + ca[10] * cb[10] + ca[11] * cb[11];
        dot_3 += ca[12] * cb[12] + ca[13] * cb[13] + ca[14] * cb[14] + ca[15] * cb[15];
    }

    // 2. Secondary 8-lane cascade
    let chunks_a8 = rem_a16.chunks_exact(8);
    let chunks_b8 = rem_b16.chunks_exact(8);
    let rem_a8 = chunks_a8.remainder();
    let rem_b8 = chunks_b8.remainder();

    for (ca, cb) in chunks_a8.zip(chunks_b8) {
        dot_0 += ca[0] * cb[0] + ca[1] * cb[1] + ca[2] * cb[2] + ca[3] * cb[3];
        dot_1 += ca[4] * cb[4] + ca[5] * cb[5] + ca[6] * cb[6] + ca[7] * cb[7];
    }

    // 3. Tertiary 4-lane cascade
    let chunks_a4 = rem_a8.chunks_exact(4);
    let chunks_b4 = rem_b8.chunks_exact(4);
    let rem_a4 = chunks_a4.remainder();
    let rem_b4 = chunks_b4.remainder();

    for (ca, cb) in chunks_a4.zip(chunks_b4) {
        dot_2 += ca[0] * cb[0] + ca[1] * cb[1] + ca[2] * cb[2] + ca[3] * cb[3];
    }

    let mut dot = (dot_0 + dot_1) + (dot_2 + dot_3);
    // 4. Final 1-3 scalar tail
    for (x, y) in rem_a4.iter().zip(rem_b4.iter()) {
        dot += x * y;
    }

    dot
}
