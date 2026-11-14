# Property Graph, GraphBLAS CSR & openCypher Support

TapirusDB stores property graphs natively inside the `.tapir` engine, combining labeled nodes, weighted typed edges, Compressed Sparse Row (`CsrGraph`) analytics, openCypher pattern queries, and hybrid GraphRAG traversal.

## Graph Engine & CSR Adjacency

- **`GraphEngine` (`src/graph/mod.rs`):** Transactional property nodes and weighted edges persisted within the database pager.
- **Compressed Sparse Row (`CsrGraph`, `src/graph/csr.rs`):** Converts graph topology into contiguous offset/edge CSR arrays (`graph_to_csr()`) for cache-friendly linear adjacency scans.

## Built-In Graph Algorithms

Exposed directly in Rust and the SQL executor (`src/sql/executor.rs`):

- **PageRank** (`graph_pagerank`) — iterative link-importance scoring across graph edges
- **Dijkstra Shortest Path** (`graph_dijkstra_path`) — weighted single-source shortest path resolution
- **Connected Components** — structural component discovery
- **Betweenness Centrality** — bridge/bottleneck node centrality scoring
- **Louvain Community Detection** — modularity-based graph clustering

## Query Interfaces & Hybrid GraphRAG

- **openCypher & SQL `GRAPH MATCH` (`src/graph/cypher.rs`):** Supports `CREATE` and `MATCH ... WHERE ... RETURN` graph pattern queries directly over embedded storage.
- **Fluent `GraphChain` (`src/lib.rs`):** Composes multi-hop `out()`, `in_()`, `both()`, edge-weight (`min_weight`), label/property filters with `vector_near()`, `bm25_search()`, and `hybrid_rank()` inside a single unified pipeline.

See [GraphRAG and Remote Storage](../GRAPHRAG_AND_REMOTE_STORAGE.md) and [API Cookbook](../COOKBOOK.md).
