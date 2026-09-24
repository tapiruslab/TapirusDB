//! Native Embedded Property Graph Engine for TapirusDB.
//!
//! Provides ultra-fast, zero-daemon graph database capabilities for AI Knowledge Graphs,
//! entity relationships, and GraphRAG pipelines within a single `.tapir` database file.

pub mod chain;
pub mod csr;
pub mod cypher;
pub mod rag;

pub use chain::{ChainMatch, GraphChain, NodeFilter, TraversalStep};
pub use csr::CsrGraph;
pub use cypher::{CypherExecutor, CypherParser, CypherStatement};
pub use rag::{GraphRagConfig, GraphRagContext, GraphRagEngine, GraphRagResult};

use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};

/// Traversal direction for graph relationships
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Direction {
    /// Follow outgoing edges from source: `(A) -> (B)`
    Outgoing,
    /// Follow incoming edges to target: `(B) <- (A)`
    Incoming,
    /// Follow edges in either direction
    Both,
}

/// A Node (Vertex) representing an entity in the Knowledge Graph
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Node {
    /// Unique 64-bit Node ID
    pub id: u64,
    /// Entity label or category (e.g., "Person", "Company", "Concept", "Paper")
    pub label: String,
    /// Flexible properties (JSON string, key-value, etc.)
    pub properties: String,
    /// Optional high-dimensional dense vector embedding
    #[serde(default)]
    pub vector: Option<Vec<f32>>,
}

/// An Edge (Relationship) connecting two nodes in the Knowledge Graph
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Edge {
    /// Unique 64-bit Edge ID
    pub id: u64,
    /// Source Node ID
    pub from_id: u64,
    /// Target Node ID
    pub to_id: u64,
    /// Edge relationship type (e.g., "KNOWS", "FOUNDED", "CITED_BY", "PART_OF")
    pub label: String,
    /// Relationship weight or confidence score (default: 1.0)
    pub weight: f32,
    /// Relationship properties
    pub properties: String,
}

/// Embedded Property Graph Engine with bi-directional adjacency indexing
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct GraphEngine {
    nodes: HashMap<u64, Node>,
    edges: HashMap<u64, Edge>,
    outgoing: HashMap<u64, Vec<u64>>, // node_id -> Vec<edge_id>
    incoming: HashMap<u64, Vec<u64>>, // node_id -> Vec<edge_id>
    next_edge_id: u64,
}

impl GraphEngine {
    /// Create a new empty GraphEngine
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            edges: HashMap::new(),
            outgoing: HashMap::new(),
            incoming: HashMap::new(),
            next_edge_id: 1,
        }
    }

    /// Add or update a Node in the graph with optional dense vector embedding
    pub fn add_node_with_vector<S1: Into<String>, S2: Into<String>>(
        &mut self,
        id: u64,
        label: S1,
        properties: S2,
        vector: Option<Vec<f32>>,
    ) -> Result<()> {
        let node = Node {
            id,
            label: label.into(),
            properties: properties.into(),
            vector,
        };
        self.nodes.insert(id, node);
        self.outgoing.entry(id).or_default();
        self.incoming.entry(id).or_default();
        Ok(())
    }

    /// Add or update a Node in the graph without a vector
    pub fn add_node<S1: Into<String>, S2: Into<String>>(
        &mut self,
        id: u64,
        label: S1,
        properties: S2,
    ) -> Result<()> {
        self.add_node_with_vector(id, label, properties, None)
    }

    /// Start a fluent Graph-to-Vector chaining query from a single node
    pub fn chain(&self, start_id: u64) -> GraphChain<'_> {
        GraphChain::new(self, vec![start_id])
    }

    /// Start a fluent Graph-to-Vector chaining query from multiple seed nodes
    pub fn chain_multi(&self, start_ids: Vec<u64>) -> GraphChain<'_> {
        GraphChain::new(self, start_ids)
    }

    /// Add a directional Edge connecting two nodes
    pub fn add_edge<S1: Into<String>, S2: Into<String>>(
        &mut self,
        from_id: u64,
        to_id: u64,
        label: S1,
        weight: f32,
        properties: S2,
    ) -> Result<u64> {
        if !self.nodes.contains_key(&from_id) {
            return Err(Error::Corrupted(format!(
                "Source node #{from_id} does not exist in graph"
            )));
        }
        if !self.nodes.contains_key(&to_id) {
            return Err(Error::Corrupted(format!(
                "Target node #{to_id} does not exist in graph"
            )));
        }

        let edge_id = self.next_edge_id;
        self.next_edge_id += 1;

        let edge = Edge {
            id: edge_id,
            from_id,
            to_id,
            label: label.into(),
            weight,
            properties: properties.into(),
        };

        self.edges.insert(edge_id, edge);
        self.outgoing.entry(from_id).or_default().push(edge_id);
        self.incoming.entry(to_id).or_default().push(edge_id);

        Ok(edge_id)
    }

    /// Remove a Node and all its connected incident edges from the graph
    pub fn remove_node(&mut self, id: u64) -> (bool, Vec<u64>) {
        if self.nodes.remove(&id).is_some() {
            let mut removed_edges = Vec::new();
            if let Some(out_edges) = self.outgoing.remove(&id) {
                for eid in out_edges {
                    if let Some(edge) = self.edges.remove(&eid) {
                        removed_edges.push(eid);
                        if let Some(in_list) = self.incoming.get_mut(&edge.to_id) {
                            in_list.retain(|&e| e != eid);
                        }
                    }
                }
            }
            if let Some(in_edges) = self.incoming.remove(&id) {
                for eid in in_edges {
                    if let Some(edge) = self.edges.remove(&eid) {
                        removed_edges.push(eid);
                        if let Some(out_list) = self.outgoing.get_mut(&edge.from_id) {
                            out_list.retain(|&e| e != eid);
                        }
                    }
                }
            }
            (true, removed_edges)
        } else {
            (false, Vec::new())
        }
    }

    /// Remove an Edge connecting two nodes
    pub fn remove_edge(&mut self, id: u64) -> bool {
        if let Some(edge) = self.edges.remove(&id) {
            if let Some(out_list) = self.outgoing.get_mut(&edge.from_id) {
                out_list.retain(|&e| e != id);
            }
            if let Some(in_list) = self.incoming.get_mut(&edge.to_id) {
                in_list.retain(|&e| e != id);
            }
            true
        } else {
            false
        }
    }

    /// Restore a node from disk storage
    pub fn restore_node(&mut self, node: Node) {
        let id = node.id;
        self.nodes.insert(id, node);
        self.outgoing.entry(id).or_default();
        self.incoming.entry(id).or_default();
    }

    /// Restore an edge from disk storage
    pub fn restore_edge(&mut self, edge: Edge) {
        let edge_id = edge.id;
        let from_id = edge.from_id;
        let to_id = edge.to_id;
        if edge_id >= self.next_edge_id {
            self.next_edge_id = edge_id + 1;
        }
        self.edges.insert(edge_id, edge);
        self.outgoing.entry(from_id).or_default().push(edge_id);
        self.incoming.entry(to_id).or_default().push(edge_id);
    }

    /// Get reference to a Node by its ID
    pub fn get_node(&self, id: u64) -> Option<&Node> {
        self.nodes.get(&id)
    }

    /// Get reference to an Edge by its ID
    pub fn get_edge(&self, id: u64) -> Option<&Edge> {
        self.edges.get(&id)
    }

    /// Total count of nodes in the graph
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Total count of edges in the graph
    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    /// Return all nodes in the graph
    pub fn all_nodes(&self) -> Vec<&Node> {
        self.nodes.values().collect()
    }

    /// Return all edges in the graph
    pub fn all_edges(&self) -> Vec<&Edge> {
        self.edges.values().collect()
    }

    /// Find adjacent neighbor references of a node without cloning Node or Edge
    pub fn neighbor_refs<'a>(
        &'a self,
        node_id: u64,
        direction: Direction,
        edge_label: Option<&str>,
    ) -> Vec<(&'a Node, &'a Edge)> {
        let mut results = Vec::new();
        let mut check_edge = |eid: u64| {
            if let Some(edge) = self.edges.get(&eid) {
                if let Some(filter_label) = edge_label {
                    if !edge.label.eq_ignore_ascii_case(filter_label) {
                        return;
                    }
                }

                let neighbor_id = if edge.from_id == node_id {
                    edge.to_id
                } else {
                    edge.from_id
                };

                if let Some(neighbor_node) = self.nodes.get(&neighbor_id) {
                    results.push((neighbor_node, edge));
                }
            }
        };

        match direction {
            Direction::Outgoing => {
                if let Some(ids) = self.outgoing.get(&node_id) {
                    for &eid in ids {
                        check_edge(eid);
                    }
                }
            }
            Direction::Incoming => {
                if let Some(ids) = self.incoming.get(&node_id) {
                    for &eid in ids {
                        check_edge(eid);
                    }
                }
            }
            Direction::Both => {
                if let Some(ids) = self.outgoing.get(&node_id) {
                    for &eid in ids {
                        check_edge(eid);
                    }
                }
                if let Some(ids) = self.incoming.get(&node_id) {
                    for &eid in ids {
                        check_edge(eid);
                    }
                }
            }
        }

        results
    }

    /// Find adjacent neighbors of a node filtered by direction and edge label
    pub fn neighbors(
        &self,
        node_id: u64,
        direction: Direction,
        edge_label: Option<&str>,
    ) -> Vec<(Node, Edge)> {
        self.neighbor_refs(node_id, direction, edge_label)
            .into_iter()
            .map(|(n, e)| (n.clone(), e.clone()))
            .collect()
    }

    /// Breadth-First Search (BFS) for shortest path between two nodes
    pub fn find_path(&self, start_id: u64, end_id: u64, max_depth: usize) -> Option<Vec<Edge>> {
        if start_id == end_id {
            return Some(Vec::new());
        }
        if !self.nodes.contains_key(&start_id) || !self.nodes.contains_key(&end_id) {
            return None;
        }

        let mut queue = VecDeque::new();
        let mut visited = HashSet::new();
        // maps node_id -> (prev_node_id, edge_id)
        let mut parent_map: HashMap<u64, (u64, u64)> = HashMap::new();

        queue.push_back((start_id, 0usize));
        visited.insert(start_id);

        let mut found = false;

        while let Some((curr_id, depth)) = queue.pop_front() {
            if curr_id == end_id {
                found = true;
                break;
            }

            if depth >= max_depth {
                continue;
            }

            if let Some(edge_ids) = self.outgoing.get(&curr_id) {
                for &eid in edge_ids {
                    if let Some(edge) = self.edges.get(&eid) {
                        let next_id = edge.to_id;
                        if visited.insert(next_id) {
                            parent_map.insert(next_id, (curr_id, eid));
                            queue.push_back((next_id, depth + 1));
                        }
                    }
                }
            }
        }

        if !found {
            return None;
        }

        // Reconstruct path backwards from end_id to start_id
        let mut path = Vec::new();
        let mut curr = end_id;
        while curr != start_id {
            if let Some(&(prev, eid)) = parent_map.get(&curr) {
                if let Some(edge) = self.edges.get(&eid) {
                    path.push(edge.clone());
                }
                curr = prev;
            } else {
                break;
            }
        }

        path.reverse();
        Some(path)
    }

    /// Extract an entity subgraph up to `depth` hops for GraphRAG context injection
    pub fn extract_subgraph(&self, center_id: u64, depth: usize) -> (Vec<Node>, Vec<Edge>) {
        let mut nodes_out = Vec::new();
        let mut edges_out = Vec::new();

        if !self.nodes.contains_key(&center_id) {
            return (nodes_out, edges_out);
        }

        let mut queue = VecDeque::new();
        let mut visited_nodes = HashSet::new();
        let mut visited_edges = HashSet::new();

        queue.push_back((center_id, 0usize));
        visited_nodes.insert(center_id);

        while let Some((curr_id, curr_depth)) = queue.pop_front() {
            if let Some(node) = self.nodes.get(&curr_id) {
                nodes_out.push(node.clone());
            }

            if curr_depth >= depth {
                continue;
            }

            // Collect outgoing edges
            if let Some(eids) = self.outgoing.get(&curr_id) {
                for &eid in eids {
                    if let Some(edge) = self.edges.get(&eid) {
                        if visited_edges.insert(eid) {
                            edges_out.push(edge.clone());
                        }
                        if visited_nodes.insert(edge.to_id) {
                            queue.push_back((edge.to_id, curr_depth + 1));
                        }
                    }
                }
            }

            // Collect incoming edges
            if let Some(eids) = self.incoming.get(&curr_id) {
                for &eid in eids {
                    if let Some(edge) = self.edges.get(&eid) {
                        if visited_edges.insert(eid) {
                            edges_out.push(edge.clone());
                        }
                        if visited_nodes.insert(edge.from_id) {
                            queue.push_back((edge.from_id, curr_depth + 1));
                        }
                    }
                }
            }
        }

        (nodes_out, edges_out)
    }

    /// Weighted Shortest Path calculation using Dijkstra's algorithm
    pub fn dijkstra_shortest_path(&self, start_id: u64, end_id: u64) -> Option<(Vec<Edge>, f32)> {
        if start_id == end_id {
            return Some((Vec::new(), 0.0));
        }
        if !self.nodes.contains_key(&start_id) || !self.nodes.contains_key(&end_id) {
            return None;
        }

        #[derive(Copy, Clone, PartialEq)]
        struct State {
            cost: f32,
            node: u64,
        }

        impl Eq for State {}

        impl Ord for State {
            fn cmp(&self, other: &Self) -> std::cmp::Ordering {
                other.cost.partial_cmp(&self.cost).unwrap_or(std::cmp::Ordering::Equal)
            }
        }

        impl PartialOrd for State {
            fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
                Some(self.cmp(other))
            }
        }

        let mut dist: HashMap<u64, f32> = HashMap::new();
        let mut heap = std::collections::BinaryHeap::new();
        let mut prev: HashMap<u64, (u64, u64)> = HashMap::new(); // node -> (prev_node, edge_id)

        dist.insert(start_id, 0.0);
        heap.push(State { cost: 0.0, node: start_id });

        let mut found = false;

        while let Some(State { cost, node }) = heap.pop() {
            if node == end_id {
                found = true;
                break;
            }

            if let Some(&d) = dist.get(&node) {
                if cost > d {
                    continue;
                }
            }

            if let Some(edge_ids) = self.outgoing.get(&node) {
                for &eid in edge_ids {
                    if let Some(edge) = self.edges.get(&eid) {
                        let next_node = edge.to_id;
                        let weight = edge.weight.max(0.0001);
                        let next_cost = cost + weight;

                        let current_best = dist.get(&next_node).copied().unwrap_or(f32::INFINITY);
                        if next_cost < current_best {
                            dist.insert(next_node, next_cost);
                            prev.insert(next_node, (node, eid));
                            heap.push(State { cost: next_cost, node: next_node });
                        }
                    }
                }
            }
        }

        if !found {
            return None;
        }

        let total_cost = *dist.get(&end_id).unwrap_or(&0.0);
        let mut path = Vec::new();
        let mut curr = end_id;
        while curr != start_id {
            if let Some(&(p_node, eid)) = prev.get(&curr) {
                if let Some(edge) = self.edges.get(&eid) {
                    path.push(edge.clone());
                }
                curr = p_node;
            } else {
                break;
            }
        }

        path.reverse();
        Some((path, total_cost))
    }

    /// Compute PageRank centrality scores for all nodes in the graph
    pub fn pagerank(
        &self,
        damping_factor: f32,
        max_iterations: usize,
        tolerance: f32,
    ) -> HashMap<u64, f32> {
        let n = self.nodes.len();
        if n == 0 {
            return HashMap::new();
        }

        let initial_rank = 1.0 / (n as f32);
        let mut ranks: HashMap<u64, f32> = self.nodes.keys().map(|&id| (id, initial_rank)).collect();

        let d = damping_factor.clamp(0.01, 0.99);
        let base_score = (1.0 - d) / (n as f32);

        for _ in 0..max_iterations {
            let mut next_ranks = HashMap::with_capacity(n);
            let mut sink_sum = 0.0f32;

            // Nodes with zero outgoing edges distribute their rank to all nodes
            for (&id, &rank) in &ranks {
                let out_degree = self.outgoing.get(&id).map(|e| e.len()).unwrap_or(0);
                if out_degree == 0 {
                    sink_sum += rank;
                }
            }

            let sink_redistribution = (d * sink_sum) / (n as f32);

            let mut diff = 0.0f32;

            for &u in self.nodes.keys() {
                let mut incoming_sum = 0.0f32;
                if let Some(incoming_edges) = self.incoming.get(&u) {
                    for &eid in incoming_edges {
                        if let Some(edge) = self.edges.get(&eid) {
                            let v = edge.from_id;
                            let out_deg = self.outgoing.get(&v).map(|e| e.len()).unwrap_or(1).max(1);
                            let v_rank = ranks.get(&v).copied().unwrap_or(0.0);
                            incoming_sum += v_rank / (out_deg as f32);
                        }
                    }
                }

                let new_rank = base_score + d * incoming_sum + sink_redistribution;
                let old_rank = ranks.get(&u).copied().unwrap_or(0.0);
                diff += (new_rank - old_rank).abs();
                next_ranks.insert(u, new_rank);
            }

            ranks = next_ranks;

            if diff < tolerance {
                break;
            }
        }

        ranks
    }

    /// Convert graph into a contiguous Compressed Sparse Row (CSR) topology
    pub fn to_csr(&self) -> CsrGraph {
        CsrGraph::from_graph_engine(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_graph_add_and_neighbors() {
        let mut graph = GraphEngine::new();

        graph.add_node(1, "Person", r#"{"name":"Faiz"}"#).unwrap();
        graph.add_node(2, "Project", r#"{"name":"TapirusDB"}"#).unwrap();
        graph.add_node(3, "Organization", r#"{"name":"Tapirus Tech Lab"}"#).unwrap();

        graph.add_edge(1, 2, "ARCHITECT_OF", 1.0, "").unwrap();
        graph.add_edge(1, 3, "FOUNDER_OF", 1.0, "").unwrap();
        graph.add_edge(2, 3, "AFFILIATED_WITH", 1.0, "").unwrap();

        assert_eq!(graph.node_count(), 3);
        assert_eq!(graph.edge_count(), 3);

        // Find outgoing neighbors of Faiz (node 1)
        let neighbors = graph.neighbors(1, Direction::Outgoing, None);
        assert_eq!(neighbors.len(), 2);

        // Filter by edge label
        let founded = graph.neighbors(1, Direction::Outgoing, Some("FOUNDER_OF"));
        assert_eq!(founded.len(), 1);
        assert_eq!(founded[0].0.label, "Organization");
    }

    #[test]
    fn test_graph_shortest_path_and_subgraph() {
        let mut graph = GraphEngine::new();

        graph.add_node(1, "A", "").unwrap();
        graph.add_node(2, "B", "").unwrap();
        graph.add_node(3, "C", "").unwrap();
        graph.add_node(4, "D", "").unwrap();

        graph.add_edge(1, 2, "CONNECTS", 1.0, "").unwrap();
        graph.add_edge(2, 3, "CONNECTS", 1.0, "").unwrap();
        graph.add_edge(3, 4, "CONNECTS", 1.0, "").unwrap();

        // Path from 1 to 4
        let path = graph.find_path(1, 4, 5).expect("Path should exist");
        assert_eq!(path.len(), 3);
        assert_eq!(path[0].from_id, 1);
        assert_eq!(path[2].to_id, 4);

        // Subgraph extraction at node 2 with depth 1
        let (nodes, edges) = graph.extract_subgraph(2, 1);
        assert_eq!(nodes.len(), 3); // 2, 1 (incoming), 3 (outgoing)
        assert_eq!(edges.len(), 2);
    }
}
