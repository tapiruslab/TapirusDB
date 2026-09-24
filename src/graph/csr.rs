//! Compressed Sparse Row (CSR) Graph Topology Engine
//!
//! Stores graph edges in contiguous arrays for high-speed sequential neighbor
//! traversal, cache locality, and worst-case optimal join (WCOJ) triangle intersections
//! in 100% Safe Rust (`#![forbid(unsafe_code)]`).

use crate::error::{Error, Result};
use crate::graph::GraphEngine;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};

/// Compressed Sparse Row (CSR) and Column (CSC) contiguous graph topology
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CsrGraph {
    /// Mapping from arbitrary 64-bit Node ID to contiguous index `0..node_count`
    pub node_id_to_idx: HashMap<u64, usize>,
    /// Mapping from contiguous index back to 64-bit Node ID
    pub idx_to_node_id: Vec<u64>,
    /// Cached node labels corresponding to contiguous indices
    pub node_labels: Vec<String>,
    /// Cached node properties
    pub node_properties: Vec<String>,

    // --- CSR (Outgoing Edges) ---
    /// Row offsets for outgoing edges (size: `node_count + 1`)
    pub outgoing_offsets: Vec<usize>,
    /// Contiguous target node IDs for outgoing edges
    pub outgoing_targets: Vec<u64>,
    /// Contiguous edge weights for outgoing edges
    pub outgoing_weights: Vec<f32>,
    /// Contiguous edge labels for outgoing edges
    pub outgoing_labels: Vec<String>,
    /// Contiguous original edge IDs
    pub outgoing_edge_ids: Vec<u64>,

    // --- CSC (Incoming Edges for bi-directional traversal) ---
    /// Column offsets for incoming edges (size: `node_count + 1`)
    pub incoming_offsets: Vec<usize>,
    /// Contiguous source node IDs for incoming edges
    pub incoming_sources: Vec<u64>,
    /// Contiguous edge IDs for incoming edges
    pub incoming_edge_ids: Vec<u64>,

    /// Total edge count
    pub edge_count: usize,
}

impl CsrGraph {
    /// Convert an existing `GraphEngine` into a high-performance contiguous `CsrGraph`
    pub fn from_graph_engine(engine: &GraphEngine) -> Self {
        let all_nodes = engine.all_nodes();
        let node_count = all_nodes.len();

        let mut node_id_to_idx = HashMap::with_capacity(node_count);
        let mut idx_to_node_id = Vec::with_capacity(node_count);
        let mut node_labels = Vec::with_capacity(node_count);
        let mut node_properties = Vec::with_capacity(node_count);

        for (idx, node) in all_nodes.into_iter().enumerate() {
            node_id_to_idx.insert(node.id, idx);
            idx_to_node_id.push(node.id);
            node_labels.push(node.label.clone());
            node_properties.push(node.properties.clone());
        }

        // Build CSR (Outgoing)
        let mut outgoing_offsets = Vec::with_capacity(node_count + 1);
        let mut outgoing_targets = Vec::new();
        let mut outgoing_weights = Vec::new();
        let mut outgoing_labels = Vec::new();
        let mut outgoing_edge_ids = Vec::new();

        let mut current_out_offset = 0;
        outgoing_offsets.push(current_out_offset);

        for &node_id in &idx_to_node_id {
            let mut neighbors = engine.neighbors(node_id, crate::graph::Direction::Outgoing, None);
            // Sort by target node ID for fast binary search in `has_edge` and set intersection
            neighbors.sort_by_key(|(n, _)| n.id);

            for (target_node, edge) in neighbors {
                outgoing_targets.push(target_node.id);
                outgoing_weights.push(edge.weight);
                outgoing_labels.push(edge.label);
                outgoing_edge_ids.push(edge.id);
                current_out_offset += 1;
            }
            outgoing_offsets.push(current_out_offset);
        }

        // Build CSC (Incoming)
        let mut incoming_offsets = Vec::with_capacity(node_count + 1);
        let mut incoming_sources = Vec::new();
        let mut incoming_edge_ids = Vec::new();

        let mut current_in_offset = 0;
        incoming_offsets.push(current_in_offset);

        for &node_id in &idx_to_node_id {
            let mut neighbors = engine.neighbors(node_id, crate::graph::Direction::Incoming, None);
            neighbors.sort_by_key(|(n, _)| n.id);

            for (source_node, edge) in neighbors {
                incoming_sources.push(source_node.id);
                incoming_edge_ids.push(edge.id);
                current_in_offset += 1;
            }
            incoming_offsets.push(current_in_offset);
        }

        Self {
            node_id_to_idx,
            idx_to_node_id,
            node_labels,
            node_properties,
            outgoing_offsets,
            outgoing_targets,
            outgoing_weights,
            outgoing_labels,
            outgoing_edge_ids,
            incoming_offsets,
            incoming_sources,
            incoming_edge_ids,
            edge_count: engine.edge_count(),
        }
    }

    /// Reconstruct a standard `GraphEngine` from this `CsrGraph`
    pub fn to_graph_engine(&self) -> GraphEngine {
        let mut engine = GraphEngine::new();

        for (idx, &id) in self.idx_to_node_id.iter().enumerate() {
            let _ = engine.add_node(id, &self.node_labels[idx], &self.node_properties[idx]);
        }

        for (u_idx, &from_id) in self.idx_to_node_id.iter().enumerate() {
            let start = self.outgoing_offsets[u_idx];
            let end = self.outgoing_offsets[u_idx + 1];

            for edge_pos in start..end {
                let to_id = self.outgoing_targets[edge_pos];
                let weight = self.outgoing_weights[edge_pos];
                let label = &self.outgoing_labels[edge_pos];
                let _ = engine.add_edge(from_id, to_id, label, weight, "");
            }
        }

        engine
    }

    /// Total count of nodes in graph
    pub fn node_count(&self) -> usize {
        self.idx_to_node_id.len()
    }

    /// Total count of edges in graph
    pub fn edge_count(&self) -> usize {
        self.edge_count
    }

    /// Retrieve contiguous slice of outgoing neighbor node IDs with zero memory allocations
    pub fn outgoing_neighbors(&self, node_id: u64) -> &[u64] {
        if let Some(&idx) = self.node_id_to_idx.get(&node_id) {
            let start = self.outgoing_offsets[idx];
            let end = self.outgoing_offsets[idx + 1];
            &self.outgoing_targets[start..end]
        } else {
            &[]
        }
    }

    /// Retrieve contiguous slice of incoming neighbor node IDs with zero memory allocations
    pub fn incoming_neighbors(&self, node_id: u64) -> &[u64] {
        if let Some(&idx) = self.node_id_to_idx.get(&node_id) {
            let start = self.incoming_offsets[idx];
            let end = self.incoming_offsets[idx + 1];
            &self.incoming_sources[start..end]
        } else {
            &[]
        }
    }

    /// Get outgoing degree of a node
    pub fn outgoing_degree(&self, node_id: u64) -> usize {
        self.outgoing_neighbors(node_id).len()
    }

    /// Get incoming degree of a node
    pub fn incoming_degree(&self, node_id: u64) -> usize {
        self.incoming_neighbors(node_id).len()
    }

    /// Fast edge existence check using binary search on the sorted neighbor slice
    pub fn has_edge(&self, from: u64, to: u64) -> bool {
        let targets = self.outgoing_neighbors(from);
        targets.binary_search(&to).is_ok()
    }

    /// Count common neighbors between two nodes using two-pointer sorted slice intersection
    pub fn common_neighbors_count(&self, u: u64, v: u64) -> usize {
        let neighbors_u = self.outgoing_neighbors(u);
        let neighbors_v = self.outgoing_neighbors(v);

        let mut i = 0;
        let mut j = 0;
        let mut count = 0;

        while i < neighbors_u.len() && j < neighbors_v.len() {
            match neighbors_u[i].cmp(&neighbors_v[j]) {
                std::cmp::Ordering::Equal => {
                    count += 1;
                    i += 1;
                    j += 1;
                }
                std::cmp::Ordering::Less => i += 1,
                std::cmp::Ordering::Greater => j += 1,
            }
        }

        count
    }

    /// Triangle counting using fast sorted CSR intersection (Worst-Case Optimal Join primitive)
    pub fn triangle_count(&self) -> usize {
        let mut total_triangles = 0;

        for (u_idx, &u) in self.idx_to_node_id.iter().enumerate() {
            let start_u = self.outgoing_offsets[u_idx];
            let end_u = self.outgoing_offsets[u_idx + 1];

            for edge_pos in start_u..end_u {
                let v = self.outgoing_targets[edge_pos];
                // Direct comparison to avoid double counting
                if v > u {
                    total_triangles += self.common_neighbors_count(u, v);
                }
            }
        }

        total_triangles / 3
    }

    /// High-throughput BFS traversal over contiguous CSR neighbor slices
    pub fn bfs(&self, start_id: u64, max_depth: usize) -> Vec<u64> {
        let mut visited = HashMap::new();
        let mut queue = VecDeque::new();
        let mut order = Vec::new();

        visited.insert(start_id, 0usize);
        queue.push_back(start_id);
        order.push(start_id);

        while let Some(current) = queue.pop_front() {
            let depth = visited[&current];
            if depth >= max_depth {
                continue;
            }

            for &neighbor in self.outgoing_neighbors(current) {
                if !visited.contains_key(&neighbor) {
                    visited.insert(neighbor, depth + 1);
                    queue.push_back(neighbor);
                    order.push(neighbor);
                }
            }
        }

        order
    }

    /// Serialize CSR structure to binary format
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        serde_json::to_vec(self)
            .map_err(|e| Error::Corrupted(format!("Failed to serialize CSR graph: {e}")))
    }

    /// Deserialize CSR structure from binary format
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        serde_json::from_slice(bytes)
            .map_err(|e| Error::Corrupted(format!("Failed to deserialize CSR graph: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_csr_roundtrip_and_slices() {
        let mut engine = GraphEngine::new();
        let _ = engine.add_node(1, "Person", r#"{"name": "Alice"}"#);
        let _ = engine.add_node(2, "Person", r#"{"name": "Bob"}"#);
        let _ = engine.add_node(3, "Person", r#"{"name": "Charlie"}"#);

        let _ = engine.add_edge(1, 2, "KNOWS", 1.0, "");
        let _ = engine.add_edge(1, 3, "KNOWS", 0.8, "");
        let _ = engine.add_edge(2, 3, "FOLLOWS", 0.5, "");

        let csr = CsrGraph::from_graph_engine(&engine);
        assert_eq!(csr.node_count(), 3);
        assert_eq!(csr.edge_count(), 3);

        // Outgoing neighbors of 1 should be [2, 3]
        let out_1 = csr.outgoing_neighbors(1);
        assert_eq!(out_1, &[2, 3]);

        // Incoming neighbors of 3 should be [1, 2]
        let in_3 = csr.incoming_neighbors(3);
        assert_eq!(in_3, &[1, 2]);

        assert!(csr.has_edge(1, 2));
        assert!(csr.has_edge(1, 3));
        assert!(!csr.has_edge(3, 1));

        let bfs_res = csr.bfs(1, 2);
        assert_eq!(bfs_res, vec![1, 2, 3]);

        // Serialization roundtrip
        let bytes = csr.to_bytes().unwrap();
        let decoded = CsrGraph::from_bytes(&bytes).unwrap();
        assert_eq!(decoded.node_count(), csr.node_count());
        assert_eq!(decoded.outgoing_neighbors(1), &[2, 3]);
    }
}
