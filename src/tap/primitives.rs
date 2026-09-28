//! The 4 Fundamental Decision Primitives of the Tap Subsystem:
//!
//! 1. `classify`: Deterministic categorical selection with confidence distribution.
//! 2. `score`: Continuous scalar alignment against criteria rubrics in $[0.0, 1.0]$.
//! 3. `verify`: Calibrated boolean truth validation with strict thresholds.
//! 4. `route`: Autonomous openCypher graph and agent workflow branch routing.

use serde::{Deserialize, Serialize};

/// Result of a categorical decision classification
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClassificationResult {
    /// The winning categorical label
    pub top_choice: String,
    /// Calibrated confidence score in $[0.0, 1.0]$
    pub confidence: f32,
    /// Ranked candidate choices with individual softmax probabilities
    pub probabilities: Vec<(String, f32)>,
    /// Shannon entropy measuring decision uncertainty (lower = more confident)
    pub entropy: f32,
    /// Latency of the evaluation in microseconds
    pub latency_us: u64,
}

/// Result of a continuous rubric evaluation
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScoreResult {
    /// Calibrated scalar score in $[0.0, 1.0]$
    pub score: f32,
    /// Normalized percentile against standard distributions
    pub normalized_percentile: f32,
    /// Key semantic signal tokens detected during evaluation
    pub matched_signals: Vec<String>,
    /// Latency of the evaluation in microseconds
    pub latency_us: u64,
}

/// Result of a boolean verification hypothesis check
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VerifyResult {
    /// Definitive boolean conclusion
    pub is_verified: bool,
    /// Confidence probability in $[0.0, 1.0]$
    pub confidence: f32,
    /// Decision threshold used for verification (e.g. 0.50)
    pub threshold: f32,
    /// Signed margin of confidence relative to the decision threshold
    pub margin: f32,
    /// Latency of the evaluation in microseconds
    pub latency_us: u64,
}

/// Result of an agent workflow or graph state branch routing
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RouteResult {
    /// The optimal target route/node identifier
    pub selected_route: String,
    /// Execution priority rank (1 = highest)
    pub priority: u32,
    /// Confidence of the route assignment in $[0.0, 1.0]$
    pub confidence: f32,
    /// Ranked alternative routes in descending order
    pub alternative_routes: Vec<String>,
    /// Latency of the evaluation in microseconds
    pub latency_us: u64,
}
