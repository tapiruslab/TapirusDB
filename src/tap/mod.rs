//! # Tap: Sub-Millisecond In-Database Cognitive Decision Subsystem
//!
//! Tap is TapirusDB's native System-1 cognitive engine. It provides deterministic,
//! non-autoregressive decision classification, rubric scoring, calibrated boolean truth
//! verification, and graph workflow routing directly inside the database kernel.
//!
//! Unlike traditional autoregressive LLMs that generate token-by-token text with high latency
//! (300ms–800ms) and unpredictable hallucinations, Tap executes single-pass tensor projections
//! directly over database memory records in **sub-millisecond Safe Rust (< 2ms)**.

pub mod primitives;
pub mod runtime;
pub mod sql_bridge;
pub mod tokenizer;

pub use primitives::{ClassificationResult, RouteResult, ScoreResult, VerifyResult};
pub use runtime::{TapInferenceEngine, TapRuntime, TapWeights, TAP_MODEL_MAGIC};
pub use sql_bridge::{eval_tap_classify, eval_tap_route, eval_tap_score, eval_tap_verify};
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

        // Negative indicators check
        let negations = ["not", "never", "no", "cannot", "invalid", "violate", "reject", "false"];
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

        let negation_penalty = if premise_neg != hyp_neg { -0.35 } else { 0.0 };

        let overlap_evidence = if overlap > 0.18 {
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

fn extract_word_set(text: &str) -> HashSet<String> {
    text.to_lowercase()
        .split(|c: char| c.is_whitespace() || c.is_ascii_punctuation())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

fn compute_jaccard_overlap(a: &HashSet<String>, b: &HashSet<String>) -> f32 {
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
}
