//! # Tap: Sub-Millisecond In-Database Cognitive Decision Subsystem
//!
//! Tap is TapirusDB's native System-1 cognitive engine. It provides deterministic,
//! non-autoregressive decision classification, rubric scoring, calibrated boolean truth
//! verification, and graph workflow routing directly inside the database kernel.
//!
//! Unlike traditional autoregressive LLMs that generate token-by-token text with high latency
//! (300ms–800ms) and unpredictable hallucinations, Tap executes single-pass tensor projections
//! directly over database memory records in **sub-millisecond Safe Rust (< 2ms)**.

pub mod bitnet;
pub mod deep;
pub mod primitives;
pub mod runtime;
pub mod sql_bridge;
pub mod tokenizer;

pub use bitnet::{BitNetBlock, BitNetLinear, BitNetRmsNorm, TernaryVal};
pub use deep::TapDeepEngine;
pub use primitives::{
    ClassificationResult, GroundedClassificationResult, GroundedVerifyResult, RouteResult,
    ScoreResult, VerifyResult,
};
pub use runtime::{TapInferenceEngine, TapRuntime, TapWeights, TAP_MODEL_MAGIC};
pub use sql_bridge::{
    eval_tap_classify, eval_tap_classify_grounded, eval_tap_route, eval_tap_score,
    eval_tap_verify, eval_tap_verify_grounded, register_grounding_index,
};
pub use tokenizer::TapTokenizer;

use crate::error::{Error, Result};
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

/// Configuration options for the Tap Decision Engine
#[derive(Debug, Clone)]
pub struct TapConfig {
    /// Latent vector dimension (default: 64)
    pub dim: usize,
    /// Default threshold for boolean verification (default: 0.50)
    pub default_verify_threshold: f32,
    /// Temperature scaling for decision softmax (default: 0.85)
    pub temperature: f32,
    /// Weight assigned to lexical semantic overlap (default: 0.40)
    pub lexical_weight: f32,
    /// Weight assigned to dense tensor semantic similarity (default: 0.60)
    pub dense_weight: f32,
}

impl Default for TapConfig {
    fn default() -> Self {
        Self {
            dim: 64,
            default_verify_threshold: 0.50,
            temperature: 0.70,
            lexical_weight: 1.20,
            dense_weight: 0.40,
        }
    }
}

/// The Tap Decision Engine instance
#[derive(Clone)]
pub struct TapEngine {
    config: TapConfig,
    tokenizer: Arc<TapTokenizer>,
    runtime: Arc<TapRuntime>,
}

impl Default for TapEngine {
    fn default() -> Self {
        Self::new(TapConfig::default())
    }
}

impl TapEngine {
    /// Initialize a new Tap engine with configuration and calibrated baseline weights
    pub fn new(config: TapConfig) -> Self {
        let tokenizer = TapTokenizer::new();
        let vocab_size = tokenizer.vocab_size();
        let mut weights = TapWeights::default_calibrated(config.dim, vocab_size);
        weights.temperature = config.temperature;

        Self {
            config,
            tokenizer: Arc::new(tokenizer),
            runtime: Arc::new(TapRuntime::new(weights)),
        }
    }

    /// Load pre-trained Tap weights from a `.tapmodel` file
    pub fn load_from_file<P: AsRef<Path>>(path: P, config: TapConfig) -> Result<Self> {
        let tokenizer = TapTokenizer::new();
        let weights = TapWeights::load_from_file(path)?;

        Ok(Self {
            config,
            tokenizer: Arc::new(tokenizer),
            runtime: Arc::new(TapRuntime::new(weights)),
        })
    }

    /// Access the internal tokenizer
    pub fn tokenizer(&self) -> &TapTokenizer {
        &self.tokenizer
    }

    /// Access the internal runtime
    pub fn runtime(&self) -> &TapRuntime {
        &self.runtime
    }

    /// Categorical classification: selects the winning choice among candidates with softmax confidence
    pub fn classify(&self, input: &str, candidates: &[&str]) -> Result<ClassificationResult> {
        let start = Instant::now();
        if candidates.is_empty() {
            return Err(Error::Internal("Tap::classify requires at least one candidate choice".into()));
        }

        let input_tokens = self.tokenizer.tokenize(input);
        let input_vec = self.runtime.forward_pool(&input_tokens);

        let input_words = extract_word_set(input);
        let mut raw_logits = Vec::with_capacity(candidates.len());

        for &cand in candidates {
            let cand_tokens = self.tokenizer.tokenize(cand);
            let cand_vec = self.runtime.forward_pool(&cand_tokens);
            let dense_sim = self.runtime.cosine_similarity(&input_vec, &cand_vec);

            let cand_words = extract_word_set(cand);
            let lexical_overlap = compute_jaccard_overlap(&input_words, &cand_words);

            let logit = (dense_sim * self.config.dense_weight) + (lexical_overlap * self.config.lexical_weight);
            raw_logits.push(logit);
        }

        let probs = self.runtime.softmax(&raw_logits);

        let mut ranked: Vec<(String, f32)> = candidates
            .iter()
            .zip(probs.iter())
            .map(|(&c, &p)| (c.to_string(), p))
            .collect();

        ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let top_choice = ranked.first().cloned().unwrap_or_else(|| ("unknown".into(), 0.0));

        let mut entropy = 0.0f32;
        for &p in &probs {
            if p > 1e-9 {
                entropy -= p * p.ln();
            }
        }

        let latency_us = start.elapsed().as_micros() as u64;

        Ok(ClassificationResult {
            top_choice: top_choice.0,
            confidence: top_choice.1,
            probabilities: ranked,
            entropy,
            latency_us,
        })
    }

    /// Rubric evaluation: scores alignment of input against criteria on a continuous $[0.0, 1.0]$ scale
    pub fn score(&self, input: &str, criteria: &str) -> Result<ScoreResult> {
        let start = Instant::now();
        let input_tokens = self.tokenizer.tokenize(input);
        let criteria_tokens = self.tokenizer.tokenize(criteria);

        let input_vec = self.runtime.forward_pool(&input_tokens);
        let criteria_vec = self.runtime.forward_pool(&criteria_tokens);

        let dense_sim = self.runtime.cosine_similarity(&input_vec, &criteria_vec);

        let input_words = extract_word_set(input);
        let criteria_words = extract_word_set(criteria);

        let mut matched_signals = Vec::new();
        for word in &criteria_words {
            if input_words.contains(word) {
                matched_signals.push(word.clone());
            }
        }

        let lexical_ratio = if criteria_words.is_empty() {
            0.0
        } else {
            matched_signals.len() as f32 / criteria_words.len() as f32
        };

        let raw_alignment = (dense_sim * self.config.dense_weight) + (lexical_ratio * self.config.lexical_weight);
        let score = self.runtime.sigmoid(raw_alignment * 3.5);
        let normalized_percentile = (score * 100.0).clamp(0.0, 100.0);
        let latency_us = start.elapsed().as_micros() as u64;

        Ok(ScoreResult {
            score,
            normalized_percentile,
            matched_signals,
            latency_us,
        })
    }

    /// Truth verification: determines whether a premise strictly verifies a hypothesis
    pub fn verify(&self, premise: &str, hypothesis: &str) -> Result<VerifyResult> {
        self.verify_with_threshold(premise, hypothesis, self.config.default_verify_threshold)
    }

    /// Truth verification with custom threshold
    pub fn verify_with_threshold(&self, premise: &str, hypothesis: &str, threshold: f32) -> Result<VerifyResult> {
        let start = Instant::now();
        let prem_tokens = self.tokenizer.tokenize(premise);
        let hyp_tokens = self.tokenizer.tokenize(hypothesis);

        let prem_vec = self.runtime.forward_pool(&prem_tokens);
        let hyp_vec = self.runtime.forward_pool(&hyp_tokens);

        let dense_sim = self.runtime.cosine_similarity(&prem_vec, &hyp_vec);

        let prem_words = extract_word_set(premise);
        let hyp_words = extract_word_set(hypothesis);

        let overlap = compute_jaccard_overlap(&prem_words, &hyp_words);

        // Pure grammatical polarity inverters across international languages
        let negations = [
            "not", "never", "untrue", "neither", "nor", "wont", "dont", "isnt", "arent", "didnt",
            "tidak", "bukan", "tak", "takde", "jangan",
            "nunca", "jamas",
            "jamais",
            "nicht", "kein", "keine",
        ];
        let mut premise_neg = false;
        let mut hyp_neg = false;
        for &neg in &negations {
            if prem_words.contains(neg) {
                premise_neg = true;
            }
            if hyp_words.contains(neg) {
                hyp_neg = true;
            }
        }

        let negation_penalty = if premise_neg != hyp_neg { -1.00 } else { 0.0 };

        let overlap_evidence = if premise_neg != hyp_neg {
            -0.60
        } else if overlap > 0.18 {
            (overlap - 0.15) * self.config.lexical_weight
        } else {
            -0.40
        };

        let raw_logit = ((dense_sim - 0.30) * self.config.dense_weight)
            + overlap_evidence
            + negation_penalty;

        let confidence = self.runtime.sigmoid(raw_logit * 3.2);
        let is_verified = confidence >= threshold;
        let margin = confidence - threshold;
        let latency_us = start.elapsed().as_micros() as u64;

        Ok(VerifyResult {
            is_verified,
            confidence,
            threshold,
            margin,
            latency_us,
        })
    }

    /// HNSW-Grounded truth verification: cross-references an empirical premise and hypothesis
    /// against an in-database HNSW vector index containing reference document chunks or policy embeddings.
    pub fn verify_grounded(
        &self,
        premise: &str,
        hypothesis: &str,
        index: &crate::vector::HnswIndex,
        top_k: usize,
    ) -> Result<GroundedVerifyResult> {
        let start = Instant::now();
        let base_verify = self.verify(premise, hypothesis)?;

        let prem_tokens = self.tokenizer.tokenize(premise);
        let mut query_vec = self.runtime.forward_pool(&prem_tokens);

        if query_vec.len() != index.dimensions {
            if query_vec.len() > index.dimensions {
                query_vec.truncate(index.dimensions);
            } else {
                query_vec.resize(index.dimensions, 0.0);
            }
        }

        let k = top_k.max(1);
        let knn_matches = index.search_knn(&query_vec, k, crate::vector::DistanceMetric::Cosine)?;

        let mut retrieved_evidence = Vec::with_capacity(knn_matches.len());
        let mut total_sim = 0.0f32;
        for (id, dist) in knn_matches {
            let sim = (1.0 - dist.clamp(0.0, 2.0) / 2.0).clamp(0.0, 1.0);
            retrieved_evidence.push((id, sim));
            total_sim += sim;
        }

        let avg_grounding = if !retrieved_evidence.is_empty() {
            total_sim / retrieved_evidence.len() as f32
        } else {
            0.5
        };

        let grounded_confidence = ((base_verify.confidence * 0.60) + (avg_grounding * 0.40)).clamp(0.0, 1.0);
        let is_verified = grounded_confidence >= base_verify.threshold;
        let margin = grounded_confidence - base_verify.threshold;
        let latency_us = start.elapsed().as_micros() as u64;

        Ok(GroundedVerifyResult {
            is_verified,
            confidence: grounded_confidence,
            threshold: base_verify.threshold,
            margin,
            retrieved_evidence,
            grounding_score: avg_grounding,
            latency_us,
        })
    }

    /// HNSW-Grounded categorical classification: cross-references candidate labels
    /// with empirical neighbor records in an in-database HNSW vector index.
    pub fn classify_grounded(
        &self,
        input: &str,
        candidates: &[&str],
        index: &crate::vector::HnswIndex,
        top_k: usize,
    ) -> Result<GroundedClassificationResult> {
        let start = Instant::now();
        let base_class = self.classify(input, candidates)?;

        let input_tokens = self.tokenizer.tokenize(input);
        let mut query_vec = self.runtime.forward_pool(&input_tokens);
        if query_vec.len() != index.dimensions {
            if query_vec.len() > index.dimensions {
                query_vec.truncate(index.dimensions);
            } else {
                query_vec.resize(index.dimensions, 0.0);
            }
        }

        let k = top_k.max(1);
        let knn_matches = index.search_knn(&query_vec, k, crate::vector::DistanceMetric::Cosine)?;

        let mut retrieved_evidence = Vec::with_capacity(knn_matches.len());
        let mut total_sim = 0.0f32;
        for (id, dist) in knn_matches {
            let sim = (1.0 - dist.clamp(0.0, 2.0) / 2.0).clamp(0.0, 1.0);
            retrieved_evidence.push((id, sim));
            total_sim += sim;
        }

        let avg_grounding = if !retrieved_evidence.is_empty() {
            total_sim / retrieved_evidence.len() as f32
        } else {
            0.5
        };

        let modulated_conf = ((base_class.confidence * 0.65) + (avg_grounding * 0.35)).clamp(0.0, 1.0);
        let latency_us = start.elapsed().as_micros() as u64;

        Ok(GroundedClassificationResult {
            top_choice: base_class.top_choice,
            confidence: modulated_conf,
            probabilities: base_class.probabilities,
            retrieved_evidence,
            grounding_score: avg_grounding,
            entropy: base_class.entropy,
            latency_us,
        })
    }

    /// Agent workflow and graph traversal branch routing
    pub fn route(&self, state: &str, route_options: &[&str]) -> Result<RouteResult> {
        let start = Instant::now();
        if route_options.is_empty() {
            return Err(Error::Internal("Tap::route requires at least one route option".into()));
        }

        let classification = self.classify(state, route_options)?;
        let selected_route = classification.top_choice;
        let confidence = classification.confidence;

        let alternative_routes: Vec<String> = classification
            .probabilities
            .into_iter()
            .filter(|(name, _)| name != &selected_route)
            .map(|(name, _)| name)
            .collect();

        let latency_us = start.elapsed().as_micros() as u64;

        Ok(RouteResult {
            selected_route,
            priority: 1,
            confidence,
            alternative_routes,
            latency_us,
        })
    }
}

pub(crate) fn extract_word_set(text: &str) -> HashSet<String> {
    text.to_lowercase()
        .split(|c: char| c.is_whitespace() || c.is_ascii_punctuation())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

pub(crate) fn compute_jaccard_overlap(a: &HashSet<String>, b: &HashSet<String>) -> f32 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let mut score = 0.0f32;
    for wa in a {
        for wb in b {
            if wa == wb {
                score += 1.0;
            } else if (wa.len() >= 4 && wb.len() >= 4) && (wa.starts_with(wb) || wb.starts_with(wa)) {
                score += 0.7;
            }
        }
    }
    let normalizer = (a.len().min(b.len()) as f32).max(1.0);
    (score / normalizer).min(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tap_classify() {
        let tap = TapEngine::default();
        let res = tap.classify("Refund requested because product was broken", &["refund", "billing_support", "sales"]).unwrap();
        assert_eq!(res.top_choice, "refund");
        assert!(res.confidence > 0.4);
    }

    #[test]
    fn test_tap_verify() {
        let tap = TapEngine::default();
        let res = tap.verify("The user has an active premium subscription", "user is active subscriber").unwrap();
        assert!(res.is_verified);
        assert!(res.confidence >= 0.50);
    }

    #[test]
    fn test_tap_score() {
        let tap = TapEngine::default();
        let res = tap.score("Server down critical emergency production failure", "emergency severity").unwrap();
        assert!(res.score > 0.5);
        assert!(res.matched_signals.contains(&"emergency".to_string()));
    }

    #[test]
    fn test_tap_route() {
        let tap = TapEngine::default();
        let res = tap.route("High risk fraudulent transfer detected: block immediately", &["block_transfer", "standard_review", "allow"]).unwrap();
        assert_eq!(res.selected_route, "block_transfer");
    }

    #[test]
    fn test_tap_throughput_and_latency_benchmark() {
        let tap = TapEngine::default();
        let iterations = 2000;

        let start = std::time::Instant::now();
        for _ in 0..iterations {
            let _ = tap.classify("Refund requested because product was broken on delivery", &["refund", "billing_support", "sales"]).unwrap();
        }
        let classify_total = start.elapsed();
        let classify_us = classify_total.as_micros() as f64 / iterations as f64;
        let classify_qps = iterations as f64 / classify_total.as_secs_f64();

        let start_v = std::time::Instant::now();
        for _ in 0..iterations {
            let _ = tap.verify("The user has an active premium subscription", "user is active subscriber").unwrap();
        }
        let verify_total = start_v.elapsed();
        let verify_us = verify_total.as_micros() as f64 / iterations as f64;
        let verify_qps = iterations as f64 / verify_total.as_secs_f64();

        println!("\n=== TAP DECISION CORE EMPIRICAL BENCHMARK ===");
        println!("Classify Latency: {:.2} µs ({:.0} decisions/sec)", classify_us, classify_qps);
        println!("Verify Latency:   {:.2} µs ({:.0} verifications/sec)", verify_us, verify_qps);
        println!("==============================================\n");

        assert!(classify_us < 1500.0, "Classify must take < 1.5 ms (sub-millisecond in release)");
        assert!(verify_us < 1500.0, "Verify must take < 1.5 ms (sub-millisecond in release)");
    }

    #[test]
    fn test_tap_multilingual_decision() {
        let tap = TapEngine::default();

        // 1. Bahasa Melayu / Pasar
        let bm_res = tap.classify(
            "Barang rosak dan pecah, nak refund balik duit",
            &["refund", "tanya_soalan", "daftar_akaun"],
        ).unwrap();
        assert_eq!(bm_res.top_choice, "refund");

        // 2. Multilingual negation check (Melayu: 'tidak')
        let neg_res = tap.verify(
            "Pengguna tidak bersetuju dengan syarat perjanjian",
            "pengguna bersetuju dengan syarat perjanjian",
        ).unwrap();
        assert!(!neg_res.is_verified, "Negation 'tidak' must prevent false truth verification");

        // 3. Spanish decision
        let es_res = tap.classify(
            "Por favor cancelar mi cuenta inmediatamente",
            &["cancelar", "soporte", "pagar"],
        ).unwrap();
        assert_eq!(es_res.top_choice, "cancelar");

        // 4. French decision
        let fr_res = tap.classify(
            "Demande urgente de remboursement pour commande perdue",
            &["remboursement", "compte", "securite"],
        ).unwrap();
        assert_eq!(fr_res.top_choice, "remboursement");
    }

    #[test]
    fn test_tap_grounded_with_hnsw() {
        use crate::traits::VectorIndexEngine;
        use crate::vector::{DistanceMetric, HnswIndex};

        let tap = TapEngine::default();
        let dim = tap.runtime.dim();
        let mut index = HnswIndex::new(dim, DistanceMetric::Cosine);

        // Populate HNSW index with policy vectors
        let tokens_refund = tap.tokenizer.tokenize("Valid warranty return policy for damaged delivery item");
        let vec_refund = tap.runtime.forward_pool(&tokens_refund);
        index.insert_vector(101, &vec_refund).unwrap();

        let tokens_fraud = tap.tokenizer.tokenize("Fraudulent stolen credit card suspicious account activity");
        let vec_fraud = tap.runtime.forward_pool(&tokens_fraud);
        index.insert_vector(202, &vec_fraud).unwrap();

        // Test Grounded Truth Verification
        let grounded_v = tap.verify_grounded(
            "Customer provided photo proof of damaged delivery item",
            "valid warranty return damaged delivery",
            &index,
            2,
        ).unwrap();

        assert!(grounded_v.is_verified);
        assert!(!grounded_v.retrieved_evidence.is_empty());
        assert_eq!(grounded_v.retrieved_evidence[0].0, 101); // Node 101 matched as closest policy
        assert!(grounded_v.grounding_score > 0.5);

        // Test Grounded Classification
        let grounded_c = tap.classify_grounded(
            "Customer claims parcel arrived broken with shattered parts",
            &["warranty_return", "fraud_investigation"],
            &index,
            2,
        ).unwrap();

        assert!(!grounded_c.retrieved_evidence.is_empty());
        assert!(grounded_c.confidence > 0.5);
    }
}
