//! SQL Bridge providing scalar function evaluation for Tap in TapirusDB queries.
//!
//! Enables SQL queries such as:
//! - `SELECT id, TAP_CLASSIFY(content, '["fraud", "legit", "review"]') AS label FROM audit_log;`
//! - `SELECT id, TAP_SCORE(report, 'safety_hazard') AS hazard_score FROM plant_inspections;`
//! - `SELECT id, TAP_VERIFY(evidence, 'guilty_plea') AS confirmed FROM case_files;`
//! - `SELECT id, TAP_ROUTE(agent_status, '["retry", "fallback", "escalate"]') AS next_step FROM tasks;`

use crate::error::Result;
use crate::tap::TapEngine;
use crate::vector::HnswIndex;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use parking_lot::RwLock;

static GLOBAL_TAP_ENGINE: OnceLock<TapEngine> = OnceLock::new();
static GROUNDING_REGISTRY: OnceLock<RwLock<HashMap<String, Arc<HnswIndex>>>> = OnceLock::new();

fn get_grounding_registry() -> &'static RwLock<HashMap<String, Arc<HnswIndex>>> {
    GROUNDING_REGISTRY.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Register a named HNSW vector index for SQL grounded decision queries
pub fn register_grounding_index(name: &str, index: Arc<HnswIndex>) {
    let mut reg = get_grounding_registry().write();
    reg.insert(name.to_lowercase(), index);
}

/// Retrieve a registered HNSW index by name
pub fn get_grounding_index(name: &str) -> Option<Arc<HnswIndex>> {
    let reg = get_grounding_registry().read();
    reg.get(&name.to_lowercase()).cloned()
}

/// Returns a shared reference to the global TapEngine instance
pub fn get_global_tap_engine() -> &'static TapEngine {
    GLOBAL_TAP_ENGINE.get_or_init(TapEngine::default)
}

/// Evaluates `TAP_CLASSIFY(input, candidates)`
pub fn eval_tap_classify(input: &str, candidates_raw: &str) -> Result<String> {
    let candidates = parse_candidates(candidates_raw);
    if candidates.is_empty() {
        return Ok("unknown".to_string());
    }
    let cand_slices: Vec<&str> = candidates.iter().map(|s| s.as_str()).collect();
    let engine = get_global_tap_engine();
    let res = engine.classify(input, &cand_slices)?;
    Ok(res.top_choice)
}

/// Evaluates `TAP_CLASSIFY_GROUNDED(input, candidates, index_name, top_k)`
pub fn eval_tap_classify_grounded(
    input: &str,
    candidates_raw: &str,
    index_name: &str,
    top_k: usize,
) -> Result<String> {
    let candidates = parse_candidates(candidates_raw);
    if candidates.is_empty() {
        return Ok("unknown".to_string());
    }
    let cand_slices: Vec<&str> = candidates.iter().map(|s| s.as_str()).collect();
    let engine = get_global_tap_engine();
    if let Some(index) = get_grounding_index(index_name) {
        let res = engine.classify_grounded(input, &cand_slices, &index, top_k)?;
        Ok(res.top_choice)
    } else {
        let res = engine.classify(input, &cand_slices)?;
        Ok(res.top_choice)
    }
}

/// Evaluates `TAP_SCORE(input, criteria)`
pub fn eval_tap_score(input: &str, criteria: &str) -> Result<f32> {
    let engine = get_global_tap_engine();
    let res = engine.score(input, criteria)?;
    Ok(res.score)
}

/// Evaluates `TAP_VERIFY(premise, hypothesis)`
pub fn eval_tap_verify(premise: &str, hypothesis: &str) -> Result<bool> {
    let engine = get_global_tap_engine();
    let res = engine.verify(premise, hypothesis)?;
    Ok(res.is_verified)
}

/// Evaluates `TAP_VERIFY_GROUNDED(premise, hypothesis, index_name, top_k)`
pub fn eval_tap_verify_grounded(
    premise: &str,
    hypothesis: &str,
    index_name: &str,
    top_k: usize,
) -> Result<bool> {
    let engine = get_global_tap_engine();
    if let Some(index) = get_grounding_index(index_name) {
        let res = engine.verify_grounded(premise, hypothesis, &index, top_k)?;
        Ok(res.is_verified)
    } else {
        let res = engine.verify(premise, hypothesis)?;
        Ok(res.is_verified)
    }
}

/// Evaluates `TAP_ROUTE(state, routes)`
pub fn eval_tap_route(state: &str, routes_raw: &str) -> Result<String> {
    let routes = parse_candidates(routes_raw);
    if routes.is_empty() {
        return Ok("default".to_string());
    }
    let route_slices: Vec<&str> = routes.iter().map(|s| s.as_str()).collect();
    let engine = get_global_tap_engine();
    let res = engine.route(state, &route_slices)?;
    Ok(res.selected_route)
}

fn parse_candidates(raw: &str) -> Vec<String> {
    let mut trimmed = raw.trim();
    if trimmed.starts_with('[') && trimmed.ends_with(']') && trimmed.len() >= 2 {
        trimmed = trimmed[1..trimmed.len() - 1].trim();
    }
    trimmed
        .split(',')
        .map(|s| {
            s.trim()
                .trim_matches(|c| c == '\'' || c == '"' || c == '[' || c == ']' || c == ' ')
                .to_string()
        })
        .filter(|s| !s.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sql_bridge_functions() {
        let label = eval_tap_classify("Urgent billing failure on credit card", "['billing', 'technical', 'sales']").unwrap();
        assert_eq!(label, "billing");

        let verified = eval_tap_verify("The customer has provided valid proof of purchase receipt", "customer has proof of purchase").unwrap();
        assert!(verified);

        let score = eval_tap_score("Critical emergency production database crash", "emergency severity").unwrap();
        assert!(score > 0.5);

        let route = eval_tap_route("Fraud risk score exceeded limit: block transaction", "block, allow, review").unwrap();
        assert_eq!(route, "block");

        // Test grounded SQL functions with a registered index
        use crate::traits::VectorIndexEngine;
        use crate::vector::{DistanceMetric, HnswIndex};
        let mut idx = HnswIndex::new(64, DistanceMetric::Cosine);
        let sample_vec = vec![0.1; 64];
        idx.insert_vector(1, &sample_vec).unwrap();
        register_grounding_index("policy_idx", Arc::new(idx));

        let g_verified = eval_tap_verify_grounded(
            "Customer claims refund for damaged goods",
            "refund for damaged goods",
            "policy_idx",
            1,
        ).unwrap();
        assert!(g_verified);

        let g_label = eval_tap_classify_grounded(
            "Customer requests immediate refund",
            "refund, sales, support",
            "policy_idx",
            1,
        ).unwrap();
        assert_eq!(g_label, "refund");
    }
}
