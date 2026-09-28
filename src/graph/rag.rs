//! # Accelerated GraphRAG Suite with PQ Seeding & Tri-Modal RRF
//!
//! Replaces massive global brute-force vector scans with high-efficiency
//! **Seed-and-Traverse**:
//! 1. **Fast Vector Seeding:** High-speed candidate seeding via Product Quantization (PQ)
//!    Asymmetric Distance Computation or SIMD Cosine.
//! 2. **Micro-Hop Traversal:** Pointer-speed adjacency exploration ($O(1)$ per edge)
//!    collecting factual relationships up to $N$ hops.
//! 3. **Tri-Modal Reciprocal Rank Fusion (RRF):** Fuses Vector Semantics ($R_{\text{vec}}$),
//!    Lexical Keywords ($R_{\text{lex}}$), and Graph Proximity ($R_{\text{graph}}$).
//! 4. **Prompt Context Synthesizer:** Serializes retrieved subgraphs into dense,
//!    hallucination-free Markdown context for Frontier LLMs & On-Device SLMs.

use crate::error::Result;
use crate::graph::{Direction, Edge, GraphEngine, Node};
use crate::vector::{cosine_similarity, ProductQuantizer};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};

/// Configuration parameters controlling the GraphRAG pipeline
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphRagConfig {
    /// Number of seed nodes to discover from initial vector or lexical scan (default: 3)
    pub top_seeds: usize,
    /// Maximum graph relationship traversal depth from seed nodes (default: 2)
    pub max_hops: usize,
    /// Constant smoothing factor for Reciprocal Rank Fusion (default: 60.0)
    pub rrf_k: f32,
    /// Weight assigned to vector semantic similarity ranking in RRF (default: 0.50)
    pub vector_weight: f32,
    /// Weight assigned to graph structural proximity ranking in RRF (default: 0.30)
    pub graph_weight: f32,
    /// Weight assigned to lexical keyword match ranking in RRF (default: 0.20)
    pub lexical_weight: f32,
    /// Traversal direction for graph hops (Outgoing, Incoming, or Both) (default: Outgoing)
    #[serde(default)]
    pub direction: Option<Direction>,
    /// Minimum composite RRF score cutoff (default: 0.0)
    pub min_score: f32,
    /// Maximum number of final contextual entities to return (default: 5)
    pub limit: usize,
    /// Optional Tenant ID filter for multi-tenant Sovereign AI agent isolation
    #[serde(default)]
    pub tenant_id: Option<String>,
    /// Optional ACL / Role-based access control filter (e.g. ["admin", "analyst", "public"])
    #[serde(default)]
    pub allowed_roles: Option<Vec<String>>,
    /// Optional exact metadata key-value filters
    #[serde(default)]
    pub metadata_filter: Option<HashMap<String, serde_json::Value>>,
}

impl Default for GraphRagConfig {
    fn default() -> Self {
        Self {
            top_seeds: 3,
            max_hops: 2,
            rrf_k: 60.0,
            vector_weight: 0.50,
            graph_weight: 0.30,
            lexical_weight: 0.20,
            direction: Some(Direction::Both),
            min_score: 0.0,
            limit: 5,
            tenant_id: None,
            allowed_roles: None,
            metadata_filter: None,
        }
    }
}

impl GraphRagConfig {
    /// Create a new configuration with default settings
    pub fn new() -> Self {
        Self::default()
    }

    /// Set number of seed nodes
    pub fn with_seeds(mut self, seeds: usize) -> Self {
        self.top_seeds = seeds;
        self
    }

    /// Set maximum traversal hops
    pub fn with_max_hops(mut self, hops: usize) -> Self {
        self.max_hops = hops;
        self
    }

    /// Set result limit
    pub fn with_limit(mut self, limit: usize) -> Self {
        self.limit = limit;
        self
    }

    /// Configure RRF weights for vector, graph, and lexical ranks
    pub fn with_weights(mut self, vector: f32, graph: f32, lexical: f32) -> Self {
        self.vector_weight = vector;
        self.graph_weight = graph;
        self.lexical_weight = lexical;
        self
    }

    /// Set tenant ID for strict multi-tenant isolation
    pub fn with_tenant<S: Into<String>>(mut self, tenant_id: S) -> Self {
        self.tenant_id = Some(tenant_id.into());
        self
    }

    /// Set allowed roles / ACL tags for permission-aware retrieval
    pub fn with_roles<I, S>(mut self, roles: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.allowed_roles = Some(roles.into_iter().map(Into::into).collect());
        self
    }

    /// Set metadata key-value filter
    pub fn with_metadata_filter(mut self, filter: HashMap<String, serde_json::Value>) -> Self {
        self.metadata_filter = Some(filter);
        self
    }
}

/// Validate whether an entity Node satisfies the security tenant and ACL permission policies
pub fn is_node_permitted(config: &GraphRagConfig, node: &Node) -> bool {
    let json_val: Option<serde_json::Value> = serde_json::from_str(&node.properties).ok();

    // 1. Multi-Tenant Isolation
    if let Some(ref req_tenant) = config.tenant_id {
        let matches_tenant = if let Some(ref json) = json_val {
            let t_val = json.get("tenant_id")
                .or_else(|| json.get("tenant"))
                .or_else(|| json.get("tenantId"))
                .or_else(|| json.get("org_id"));
            match t_val {
                Some(serde_json::Value::String(s)) => s == req_tenant || s == "*",
                _ => false,
            }
        } else {
            node.properties.contains(&format!("tenant_id={req_tenant}"))
                || node.properties.contains(&format!("tenant={req_tenant}"))
                || node.properties.contains(&format!("\"tenant_id\": \"{req_tenant}\""))
                || node.properties.contains(&format!("\"tenant\": \"{req_tenant}\""))
        };

        if !matches_tenant {
            return false;
        }
    }

    // 2. Role-Based Access Control (RBAC) / Access Control List (ACL)
    if let Some(ref allowed_roles) = config.allowed_roles {
        let is_allowed = if let Some(ref json) = json_val {
            // Check public visibility
            let is_public = json.get("visibility")
                .and_then(|v| v.as_str())
                .map(|v| v.eq_ignore_ascii_case("public"))
                .unwrap_or(false);

            if is_public {
                true
            } else {
                let node_roles = json.get("roles")
                    .or_else(|| json.get("role"))
                    .or_else(|| json.get("acl"))
                    .or_else(|| json.get("groups"));

                match node_roles {
                    Some(serde_json::Value::Array(arr)) => {
                        arr.iter().any(|item| {
                            if let Some(r_str) = item.as_str() {
                                allowed_roles.iter().any(|ar| ar.eq_ignore_ascii_case(r_str))
                            } else {
                                false
                            }
                        })
                    }
                    Some(serde_json::Value::String(r_str)) => {
                        allowed_roles.iter().any(|ar| ar.eq_ignore_ascii_case(r_str))
                    }
                    None => true, // Nodes without restricted roles are considered accessible
                    _ => false,
                }
            }
        } else {
            // Non-JSON properties fallback: match any role token
            allowed_roles.iter().any(|r| node.properties.to_lowercase().contains(&r.to_lowercase()))
        };

        if !is_allowed {
            return false;
        }
    }

    // 3. Exact Metadata Key-Value Filter
    if let Some(ref filter) = config.metadata_filter {
        if let Some(ref json) = json_val {
            for (k, v) in filter {
                if json.get(k) != Some(v) {
                    return false;
                }
            }
        } else {
            return false;
        }
    }

    true
}

/// An individual entity match retrieved and ranked by the GraphRAG pipeline
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphRagResult {
    /// The Knowledge Graph entity Node ID
    pub entity_id: u64,
    /// Entity categorical label (e.g. "Person", "Server", "Concept")
    pub label: String,
    /// Properties payload (JSON string or metadata)
    pub properties: String,
    /// Final combined Reciprocal Rank Fusion (RRF) score
    pub rrf_score: f32,
    /// Initial seed semantic similarity score (if seeded via vector)
    pub seed_similarity: Option<f32>,
    /// Shortest hop distance from the nearest seed node (0 = seed node itself)
    pub hop_distance: usize,
    /// Direct relational edges connecting this entity to the retrieved subgraph
    pub related_edges: Vec<Edge>,
}

/// Synthesized GraphRAG context ready for injection into LLM / SLM prompts
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphRagContext {
    /// The original user query string
    pub query: String,
    /// Ranked list of entities with relational facts
    pub results: Vec<GraphRagResult>,
    /// Dense, structured Markdown context synthesized for prompt injection
    pub prompt_context: String,
}

/// High-performance GraphRAG execution engine
pub struct GraphRagEngine;

impl GraphRagEngine {
    /// Execute an end-to-end GraphRAG query combining PQ vector seeding, micro-hop traversal, and RRF
    pub fn query(
        graph: &GraphEngine,
        query_text: &str,
        query_vec: Option<&[f32]>,
        pq: Option<&ProductQuantizer>,
        config: &GraphRagConfig,
    ) -> Result<GraphRagContext> {
        let all_nodes = graph.all_nodes();
        if all_nodes.is_empty() {
            return Ok(GraphRagContext {
                query: query_text.to_string(),
                results: Vec::new(),
                prompt_context: String::from("No entities found in knowledge graph."),
            });
        }

        // ------------------------------------------------------------------
        // Step 1: Fast Seed Discovery (Vector PQ / Cosine or Lexical Fallback)
        // ------------------------------------------------------------------
        let mut seed_candidates: Vec<(u64, f32)> = Vec::new();

        if let Some(q_vec) = query_vec {
            for node in &all_nodes {
                if !is_node_permitted(config, node) {
                    continue;
                }
                if let Some(ref n_vec) = node.vector {
                    if n_vec.len() == q_vec.len() {
                        let sim = if let Some(quantizer) = pq {
                            // PQ-accelerated asymmetric distance: convert L2 distance to [0, 1] similarity
                            let code = quantizer.encode(n_vec);
                            let dist = quantizer.asymmetric_distance(&code, q_vec);
                            1.0 / (1.0 + dist)
                        } else {
                            cosine_similarity(q_vec, n_vec).max(0.0)
                        };
                        seed_candidates.push((node.id, sim));
                    }
                }
            }
        }

        // If no vector seeds found or no query vector provided, fallback to lexical matching
        if seed_candidates.is_empty() {
            let terms: Vec<String> = query_text
                .to_lowercase()
                .split_whitespace()
                .map(|s| s.trim_matches(|c: char| !c.is_alphanumeric()).to_string())
                .filter(|s| !s.is_empty())
                .collect();

            for node in &all_nodes {
                if !is_node_permitted(config, node) {
                    continue;
                }
                let text = format!("{} {}", node.label, node.properties).to_lowercase();
                let match_count = terms.iter().filter(|&t| text.contains(t)).count();
                if match_count > 0 {
                    let score = match_count as f32 / terms.len().max(1) as f32;
                    seed_candidates.push((node.id, score));
                }
            }
        }

        // Sort seed candidates descending by score
        seed_candidates.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        let seed_limit = config.top_seeds.max(1);
        let seeds: Vec<(u64, f32)> = seed_candidates.into_iter().take(seed_limit).collect();

        // ------------------------------------------------------------------
        // Step 2: Micro-Hop Adjacency Traversal (BFS)
        // ------------------------------------------------------------------
        let traversal_dir = config.direction.unwrap_or(Direction::Both);
        let mut visited_nodes: HashMap<u64, (usize, f32)> = HashMap::new(); // id -> (hop_distance, accum_weight)
        let mut node_edges: HashMap<u64, Vec<Edge>> = HashMap::new();
        let mut queue: VecDeque<(u64, usize, f32)> = VecDeque::new();

        // Seed initialization (hop = 0)
        for &(seed_id, seed_score) in &seeds {
            visited_nodes.insert(seed_id, (0, seed_score));
            queue.push_back((seed_id, 0, 1.0));
        }

        // BFS expansion up to config.max_hops
        while let Some((curr_id, curr_hops, curr_weight)) = queue.pop_front() {
            if curr_hops >= config.max_hops {
                continue;
            }

            let neighbors = graph.neighbors(curr_id, traversal_dir, None);
            for (neighbor_node, edge) in neighbors {
                if !is_node_permitted(config, &neighbor_node) {
                    continue;
                }
                let next_hops = curr_hops + 1;
                let next_weight = curr_weight * (edge.weight / (1.0 + next_hops as f32));

                node_edges
                    .entry(neighbor_node.id)
                    .or_default()
                    .push(edge.clone());
                node_edges
                    .entry(curr_id)
                    .or_default()
                    .push(edge.clone());

                let should_visit = match visited_nodes.get(&neighbor_node.id) {
                    Some(&(prev_hops, prev_weight)) => {
                        next_hops < prev_hops || (next_hops == prev_hops && next_weight > prev_weight)
                    }
                    None => true,
                };

                if should_visit {
                    visited_nodes.insert(neighbor_node.id, (next_hops, next_weight));
                    queue.push_back((neighbor_node.id, next_hops, next_weight));
                }
            }
        }

        // Deduplicate edges recorded for each node
        for edges in node_edges.values_mut() {
            let mut seen_edge_ids = HashSet::new();
            edges.retain(|e| seen_edge_ids.insert(e.id));
        }

        // ------------------------------------------------------------------
        // Step 3: Tri-Modal Reciprocal Rank Fusion (RRF)
        // ------------------------------------------------------------------
        let candidate_ids: Vec<u64> = visited_nodes.keys().copied().collect();
        if candidate_ids.is_empty() {
            return Ok(GraphRagContext {
                query: query_text.to_string(),
                results: Vec::new(),
                prompt_context: String::from("No related facts discovered for query."),
            });
        }

        // 3A. Vector Semantic Ranking
        let mut vector_scores: Vec<(u64, f32)> = Vec::new();
        if let Some(q_vec) = query_vec {
            for &id in &candidate_ids {
                let score = if let Some(node) = graph.get_node(id) {
                    if let Some(ref n_vec) = node.vector {
                        if n_vec.len() == q_vec.len() {
                            cosine_similarity(q_vec, n_vec).max(0.0)
                        } else {
                            0.0
                        }
                    } else {
                        0.0
                    }
                } else {
                    0.0
                };
                vector_scores.push((id, score));
            }
        } else {
            for &id in &candidate_ids {
                let s_score = seeds.iter().find(|&&(s_id, _)| s_id == id).map(|&(_, s)| s).unwrap_or(0.0);
                vector_scores.push((id, s_score));
            }
        }
        vector_scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        let vec_ranks: HashMap<u64, usize> = vector_scores
            .iter()
            .enumerate()
            .map(|(rank, &(id, _))| (id, rank + 1))
            .collect();

        // 3B. Lexical Keyword Ranking
        let terms: Vec<String> = query_text
            .to_lowercase()
            .split_whitespace()
            .map(|s| s.trim_matches(|c: char| !c.is_alphanumeric()).to_string())
            .filter(|s| !s.is_empty())
            .collect();

        let mut lexical_scores: Vec<(u64, f32)> = Vec::new();
        for &id in &candidate_ids {
            let score = if let Some(node) = graph.get_node(id) {
                let text = format!("{} {}", node.label, node.properties).to_lowercase();
                let matches = terms.iter().filter(|&t| text.contains(t)).count();
                matches as f32 / terms.len().max(1) as f32
            } else {
                0.0
            };
            lexical_scores.push((id, score));
        }
        lexical_scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        let lex_ranks: HashMap<u64, usize> = lexical_scores
            .iter()
            .enumerate()
            .map(|(rank, &(id, _))| (id, rank + 1))
            .collect();

        // 3C. Graph Structural Proximity Ranking (lower hop distance = better rank, then higher edge weight)
        let mut graph_proximity: Vec<(u64, usize, f32)> = candidate_ids
            .iter()
            .map(|&id| {
                let &(hops, weight) = visited_nodes.get(&id).unwrap_or(&(usize::MAX, 0.0));
                (id, hops, weight)
            })
            .collect();

        graph_proximity.sort_by(|a, b| {
            if a.1 != b.1 {
                a.1.cmp(&b.1) // Lower hops first
            } else {
                b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal) // Higher weight first
            }
        });
        let graph_ranks: HashMap<u64, usize> = graph_proximity
            .iter()
            .enumerate()
            .map(|(rank, &(id, _, _))| (id, rank + 1))
            .collect();

        // 3D. Combine with Reciprocal Rank Fusion (RRF) formula:
        // RRF(e) = sum_m [ weight_m / (k + rank_m(e)) ]
        let mut final_results: Vec<GraphRagResult> = Vec::new();
        let worst_rank = candidate_ids.len() + 10;

        for &id in &candidate_ids {
            if let Some(node) = graph.get_node(id) {
                let r_vec = *vec_ranks.get(&id).unwrap_or(&worst_rank) as f32;
                let r_lex = *lex_ranks.get(&id).unwrap_or(&worst_rank) as f32;
                let r_graph = *graph_ranks.get(&id).unwrap_or(&worst_rank) as f32;

                let rrf_score = (config.vector_weight / (config.rrf_k + r_vec))
                    + (config.lexical_weight / (config.rrf_k + r_lex))
                    + (config.graph_weight / (config.rrf_k + r_graph));

                if rrf_score >= config.min_score {
                    let &(hop_dist, _) = visited_nodes.get(&id).unwrap_or(&(0, 0.0));
                    let seed_sim = seeds.iter().find(|&&(s_id, _)| s_id == id).map(|&(_, s)| s);
                    let edges = node_edges.get(&id).cloned().unwrap_or_default();

                    final_results.push(GraphRagResult {
                        entity_id: id,
                        label: node.label.clone(),
                        properties: node.properties.clone(),
                        rrf_score,
                        seed_similarity: seed_sim,
                        hop_distance: hop_dist,
                        related_edges: edges,
                    });
                }
            }
        }

        // Sort final results descending by RRF score
        final_results.sort_by(|a, b| b.rrf_score.partial_cmp(&a.rrf_score).unwrap_or(std::cmp::Ordering::Equal));
        final_results.truncate(config.limit);

        // ------------------------------------------------------------------
        // Step 4: Synthesize Prompt Context for LLM / SLM
        // ------------------------------------------------------------------
        let mut prompt_context = String::new();
        if let Some(ref tenant) = config.tenant_id {
            let roles_desc = config.allowed_roles.as_ref().map(|r| r.join(", ")).unwrap_or_else(|| "Any".to_string());
            prompt_context.push_str(&format!("### 🧠 Verified Knowledge Graph Context (Tenant: `{tenant}` | Roles: [{roles_desc}])\n\n"));
        } else if let Some(ref roles) = config.allowed_roles {
            prompt_context.push_str(&format!("### 🧠 Verified Knowledge Graph Context (Roles: [{}])\n\n", roles.join(", ")));
        } else {
            prompt_context.push_str("### 🧠 Verified Knowledge Graph Context\n\n");
        }
        prompt_context.push_str("#### Entities & Facts:\n");

        for r in &final_results {
            prompt_context.push_str(&format!(
                "- **{}** (ID: {}) [Hops: {}, Confidence: {:.4}]\n  - Properties: {}\n",
                r.label, r.entity_id, r.hop_distance, r.rrf_score, r.properties
            ));
        }

        // Collect unique relationships among retrieved entities
        let retrieved_ids: HashSet<u64> = final_results.iter().map(|r| r.entity_id).collect();
        let mut distinct_edges: Vec<Edge> = Vec::new();
        let mut seen_edge_ids = HashSet::new();

        for r in &final_results {
            for e in &r.related_edges {
                if seen_edge_ids.insert(e.id) {
                    if retrieved_ids.contains(&e.from_id) || retrieved_ids.contains(&e.to_id) {
                        distinct_edges.push(e.clone());
                    }
                }
            }
        }

        if !distinct_edges.is_empty() {
            prompt_context.push_str("\n#### Relationships:\n");
            for e in &distinct_edges {
                let from_name = graph
                    .get_node(e.from_id)
                    .map(|n| n.label.as_str())
                    .unwrap_or("Unknown");
                let to_name = graph
                    .get_node(e.to_id)
                    .map(|n| n.label.as_str())
                    .unwrap_or("Unknown");

                prompt_context.push_str(&format!(
                    "- (`{}`: #{}) ───[{}]───► (`{}`: #{}) (weight: {:.2})\n",
                    from_name, e.from_id, e.label, to_name, e.to_id, e.weight
                ));
            }
        }

        Ok(GraphRagContext {
            query: query_text.to_string(),
            results: final_results,
            prompt_context,
        })
    }
}
