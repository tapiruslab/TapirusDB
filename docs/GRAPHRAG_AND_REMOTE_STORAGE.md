# GraphRAG & Remote Range Storage

TapirusDB unifies property-graph traversal, HNSW vector search, and BM25 lexical scoring in a single embedded runtime while offering pluggable remote range-storage interfaces (`src/pager/remote.rs`) for object/blob-backed deployments.

## Hybrid GraphRAG Pipeline

- **Fluent Multi-Hop + Vector + BM25 Traversal (`GraphChain`):** Start from seed node IDs, filter by vertex labels/properties and minimum edge weights across `.out()`, `.in_()`, and `.both()` hops, then fuse vector similarity (`vector_near`), full-text BM25 (`bm25_search`), or weighted hybrid scoring (`hybrid_rank`).
- **Associative AI Memory Linking:** Connect episodic or semantic AI Memory records via `memory_link(id_a, id_b)` so hybrid retrieval surfaces graph-connected context alongside vector and lexical matches.
- **SQL & Cypher Interoperability:** Query graph relationships directly alongside relational tables and vector columns using `GRAPH MATCH` / `query_cypher`.

## Remote Range Storage Architecture

- `src/pager/remote.rs` provides byte-range page fetching for HTTP/object-storage-backed read replicas and read-only snapshots (`Connection::open_read_only*`), separating local page caching from remote page transport.
- For production cloud deployments, pair remote read-only snapshots with TLS transport (`rustls` + `webpki-roots`), bounded retry/timeout budgets, and validated `tapirus` backup/restore workflows.

See [GraphBLAS & openCypher Architecture](architecture/04_GRAPHBLAS_AND_OPENCYPHER.md) and [API Cookbook](COOKBOOK.md).
