# Vector Search, HNSW & Quantization Architecture

TapirusDB natively integrates vector similarity search directly inside relational SQL queries, graph traversals, and transactional AI Memory without an external sidecar vector service.

## Core Vector Engine Components

- **Exact & HNSW Nearest-Neighbor Indexing:** Supports exact brute-force search as well as hierarchical navigable small world (`HNSW`) indexes with configurable `M`, `ef_construction`, and `ef_search` traversal beams.
- **Distance Metrics & SIMD Acceleration (`--features simd`):** Implements Cosine similarity, Squared Euclidean ($L_2^2$), and Dot Product metrics, with optional fixed-width SIMD kernels via the `wide` crate and Safe-Rust chunked vector loops (`src/vector/simd.rs`).
- **Disk-Backed Paged HNSW & Memory Snapshots:** Includes paged disk-backed vector storage primitives (`src/vector/paged_hnsw.rs`) alongside persisted CRC32-validated HNSW index snapshots (`__sys_memory_index`) for instant reopen on AI Memory collections.

## Vector Quantization

- **8-Bit Scalar Quantization (`QuantizedVector8`):** Compresses `f32` embedding vectors to compact `u8` payloads with per-vector min/step scale metadata (`4x` payload reduction).
- **RaBit Bit-Packed Quantization (`RaBitQuantizer`):** Applies deterministic sign-flip rotation followed by a normalized Walsh-Hadamard transform and 1-bit sign-bit packing with exact norm preservation, enabling fast Hamming/bit-packed similarity estimation across high-dimensional embeddings.

## SQL & Multi-Model Integration

- Columns declared with `VECTOR(D)` validate dimensionality on `INSERT`, `UPDATE`, and `UPSERT`, and synchronize secondary vector indexes within SQL transactions.
- Vector similarity integrates directly into `SELECT` (`ORDER BY COSINE_DISTANCE(...)`), fluent `GraphChain::vector_near()`, and hybrid RRF / weighted `searchMemories` retrieval.

See [AI and Embedded Workflows](../AI_EMBEDDED_WORKFLOWS.md) and [Release Validation](../RELEASE_VALIDATION.md) for HNSW recall verification and benchmark methodology.
