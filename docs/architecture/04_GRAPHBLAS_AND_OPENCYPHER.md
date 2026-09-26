# Deep Architecture Guide: Compressed Sparse Row (CSR) & openCypher Graph Engine
==================================================================================

> **Author**: Ahmad Faiz • Tapirus Tech Lab  
> **Status**: Production Reference Specification  
> **Target Audience**: Graph Engineers, Knowledge System Designers, and Core Contributors

---

## 1. Executive Summary

TapirusDB implements a dual-mode graph processing architecture:
1. **Dynamic Relational Mutation Mode**: Nodes and relationships can be incrementally inserted, updated, and deleted using transactional SQL and graph DDL statements.
2. **Compressed Sparse Row (CSR) High-Throughput Matrix Mode**: Contiguous adjacency arrays optimized for zero-copy memory traversal, linear-algebraic GraphBLAS semiring sweeps, and advanced graph topology algorithms.

```
                          GRAPH MATCH Query / Algorithm
                                       │
                                       ▼
                       AST Compilation & Optimization
                                       │
                     ┌─────────────────┴─────────────────┐
                     ▼                                   ▼
        1-Hop / 2-Hop Traversal               Matrix / GraphBLAS Kernel
      (Zero-Copy Slice Sweeps)              (Louvain, Centrality, PageRank)
                     │                                   │
                     └─────────────────┬─────────────────┘
                                       ▼
                     CSR Contiguous Adjacency Representation
                     (row_ptr: [usize], col_idx: [u64], weights: [f32])
```

---

## 2. Compressed Sparse Row (CSR) Physical Layout

Property graphs in traditional graph databases (such as Neo4j) rely on pointer-heavy doubly linked lists for every node and relationship. This induces catastrophic CPU cache misses during deep traversals.

TapirusDB compresses the topological graph structure into three contiguous, cache-aligned arrays:

```
  Graph Topology:
  Node 0 ──(1.0)──► Node 1
  Node 0 ──(0.8)──► Node 2
  Node 1 ──(0.5)──► Node 2

  CSR Representation:
  row_ptr:  [ 0,       2,       3,    3 ]
              ▲        ▲        ▲     ▲
              Node 0   Node 1   Node 2 End
  col_idx:  [ 1,   2,  2 ]
  weights:  [ 1.0, 0.8, 0.5 ]
```

* **`row_ptr` (Vertex Offsets)**: Size $V + 1$. An index into `col_idx` marking the start of outgoing edges for vertex $i$. The degree of node $i$ is calculated in $O(1)$: $\text{deg}(i) = \text{row\_ptr}[i+1] - \text{row\_ptr}[i]$.
* **`col_idx` (Target Neighbor IDs)**: Size $E$. Contiguous sequence of target vertex IDs.
* **`weights` (Edge Attributes)**: Size $E$. 32-bit floating-point weights aligned alongside `col_idx`.

### Traversal Performance Guarantee
Finding all outgoing neighbors of Node $i$ requires **zero memory allocations**:
```rust
let start = row_ptr[i];
let end = row_ptr[i + 1];
let neighbors: &[u64] = &col_idx[start..end]; // Direct slice view in CPU L1/L2 cache
```
This slice sweep executes in **sub-microsecond time** ($0.55\ \mu\text{s}$ for a 1-hop neighborhood).

---

## 3. openCypher Declarative Pattern Matching

TapirusDB parses and executes standard openCypher queries over property graphs:

```cypher
GRAPH MATCH (p:Person)-[r:KNOWS]->(f:Person)
WHERE f.city = 'Kuala Lumpur'
RETURN p.name, f.name, r.weight;
```

### 3.1 Execution Pipeline
1. **Lexing & Tokenization**: Deconstructs ASCII graph syntax (`()`, `->`, `-[...]->`).
2. **Variable Binding**: Binds source variable (`p`) and target variable (`f`).
3. **Index Scan / Seed**: If an indexed property predicate exists in the `WHERE` clause (e.g. `f.city = 'Kuala Lumpur'`), the engine uses the B+Tree secondary index to seed candidate start nodes instead of executing a full graph scan.
4. **Adjacency Expansion**: Sweeps CSR neighbor slices matching the edge label filter (`KNOWS`).
5. **Projection**: Emits projected attributes directly into the standard SQL query engine format.

---

## 4. Built-in Graph Algorithms

TapirusDB embeds advanced topological algorithms directly into the query executor via `GRAPH ALGORITHM <NAME>` SQL syntax:

### 4.1 Louvain Modularity Community Detection
Identifies organic clusters and topic communities by iteratively maximizing modularity $Q$:
$$\Delta Q = \frac{k_{i,in}}{2m} - \frac{\Sigma_{tot} \cdot k_i}{4m^2}$$
* **Time Complexity**: $O(E)$ per pass, converging rapidly in 3–5 iterations.
* **Use Case**: Segmenting autonomous AI agent concepts, conversation topics, and customer personas into dense semantic clusters.

### 4.2 Brandes' Betweenness Centrality
Measures the influence of a node over information flow between all pairs of nodes:
$$C_B(v) = \sum_{s \neq v \neq t} \frac{\sigma_{st}(v)}{\sigma_{st}}$$
* **Implementation**: Forward Breadth-First Search (BFS) combined with backward pair-dependency accumulation.
* **Use Case**: Detecting mission-critical bottleneck nodes in logistics, infrastructure, and agent decision networks.

### 4.3 Weakly Connected Components (WCC)
Identifies all isolated subgraphs and network partitions using label propagation.
* **Use Case**: Ensuring graph connectivity and identifying orphaned knowledge nodes.

### 4.4 PageRank
Iterative stationary distribution over random-walk graph transitions:
$$\mathbf{p} = \frac{1-d}{N}\mathbf{1} + d \mathbf{M} \mathbf{p} \quad (d = 0.85)$$
* **Use Case**: Objective authority scoring of documents, websites, and entities.
