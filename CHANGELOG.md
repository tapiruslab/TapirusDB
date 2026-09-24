# Changelog

All notable changes to **TapirusDB** will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [0.1.3] - 2026-09-22

### Added
- **Wave 13 — Industrial Supremacy**:
  - **SQL Subqueries & Common Table Expressions (CTEs)**:
    - Added `WITH cte AS (SELECT ...) [, ...] SELECT ...` supporting multiple comma-separated CTEs, explicit column projection aliases, and nested subquery evaluations (`WHERE col [NOT] IN (SELECT ...)`).
  - **Transparent Safe-Rust Page Compression (LZ4)**:
    - Implemented pure Safe Rust (`#![forbid(unsafe_code)]`) zero-dependency LZ4 block compression engine in `src/pager/compression.rs`.
    - Integrated transparent page frame compression in `src/pager/mod.rs` reducing 4KB pages to ~1.2KB–1.8KB before encryption and disk/WAL writes, with transparent on-demand decompression on page cache load.
    - Added `conn.enable_compression()` and `conn.is_compressed()` APIs.
  - **Disk-Backed Paged HNSW Storage Primitive (DiskANN Hybrid)**:
    - Implemented `PagedVectorStore` and `PagedHnswIndex` in `src/vector/paged_hnsw.rs` decoupling upper navigational graph layers (retained in fast RAM, ~1-5% nodes) from Layer 0 node embeddings and adjacency lists stored in slotted disk pages.
    - Integrated bounded Least Recently Used (LRU) node memory cache enabling scale beyond physical RAM.
  - **SQL Native Graph Predicate Filtering**:
    - Extended `GRAPH TRAVERSE` syntax to support SQL `WHERE` clauses (`GRAPH TRAVERSE FROM <id> [OUTGOING|INCOMING|BOTH] [LABEL '<label>'] [MAX_DEPTH <n>] [WHERE <predicate>]`).
    - Evaluates target node JSON properties and edge weights (`WHERE target.age > 25 AND edge.weight >= 0.5`).
  - **Micro-Columnar Vectorized Aggregation Engine**:
    - Implemented `VectorizedAccumulator` in `src/sql/vectorized.rs` providing fast-path 4x loop-unrolled primitive scans for `SUM`, `AVG`, `MIN`, `MAX`, and `COUNT` without per-row `Row` struct allocations.
  - **SQLite C ABI Drop-in Compatibility Layer**:
    - Exported standard SQLite C ABI symbols in `crates/tapirus-ffi/src/lib.rs`: `sqlite3_open_v2`, `sqlite3_prepare_v2`, `sqlite3_step`, `sqlite3_column_*`, `sqlite3_close`, `sqlite3_errmsg`, `sqlite3_changes`, `sqlite3_libversion`.
    - Created standard C header file `include/sqlite3.h` allowing ORMs (Prisma, SQLAlchemy, Knex) and SQLite drivers to connect to TapirusDB as a drop-in replacement.
  - **Tapirus Studio Desktop Native Packaging (Tauri v2)**:
    - Configured standalone Tauri v2 native desktop application packaging in `studio/src-tauri` with native windowing and local TapirusDB engine bindings producing <10MB standalone executables.

---

## [0.1.2] - 2026-09-20

- **Tier B Bare-Metal Microcontroller & Embedded Silicon Engine**:
  - Implemented `FlashBlockDevice` and `RamBlockDevice` storage abstractions operating without an operating system file system on raw SPI NOR Flash (W25Q128/W25Q64) and static SRAM partitions.
  - Implemented `MicroPager` using 512-byte micro-blocks with CRC32 checksums and dual-header power-loss recovery.
  - Implemented `MicroDatabase` for sub-512KB SRAM environments supporting deterministic sensor telemetry logging, 8-bit quantized micro-vectors ($L_2$ distance), and entity-to-entity edge adjacency graph traversal.
- **Robotics & Autonomous Automotive Guide**:
  - Published technical guide in `docs/ROBOTICS_AUTOMOTIVE_EDGE.md` defining the 3-Tier Hardware Continuum (Tier A: Linux/ROS2 ADAS, Tier B: Bare-Metal MCU ASIL-D/MISRA-C, Tier C: Swarm Fleet Multi-Raft Mesh) with ROS2 C++ perception nodes and real-time black box flight recorder patterns.
- **Official Package Distributions**:
  - Published official TypeScript and JavaScript SDK under NPM (`@tapirus/db`) with native CDC event listeners.
  - Published Homebrew formula (`Formula/tapirus.rb`) for macOS and Linux CLI installations.
  - Published Windows Package Manager (Winget) manifest (`winget/tapirus.yaml`).
- **Next-Gen RAG & Quantization Suite**:
  - **Accelerated GraphRAG (PQ Seeding + Micro-Hop + Tri-Modal RRF)**: High-efficiency `GraphRagEngine` replacing global brute-force vector scans with fast Product Quantization seed matching, micro-hop graph adjacency traversal, and tri-modal RRF fusing vector, lexical, and graph structural proximity scores with prompt-ready context synthesis.
  - **Product Quantization (PQ)**: Implemented `ProductQuantizer` and `QuantizedVectorPQ` delivering 16x–32x RAM reduction with asymmetric distance calculations.
  - **Reciprocal Rank Fusion (RRF)**: Implemented `reciprocal_rank_fusion` and `memory_recall_rrf` uniting BM25 inverted index keywords and dense vector similarity.
  - **Built-in Embedder Engine**: Integrated `DeterministicHashEmbedder` and `EmbeddingEngine` trait providing zero-dependency text-to-vector generation.
- **Storage, Durability, S3/R2 Remote Streaming & Time-Travel**:
  - **S3 / Cloudflare R2 Remote Storage Adapter (`RemotePager`)**: Cloud-native on-demand 4KB page streaming using HTTP Range requests (`bytes=offset-(offset+4095)`) directly against S3/R2 with in-memory page caching.
  - **Cloudflare Worker Serverless Edge Adapter**: Complete serverless edge adapter in `examples/cloudflare-worker/` demonstrating sub-5ms cold-start SQL, GraphRAG, and Agent Memory in V8 worker isolates.
  - **Hot Online Backup (`VACUUM INTO 'target.tapir'`)**: Supported non-blocking database cloning and page compaction to target files.
  - **Time-Travel Querying (`SELECT ... AS OF TIMESTAMP <ts>`)**: Added AST parser and historical point-in-time timestamp evaluation.
  - **WASM OPFS Persistence**: Added Origin Private File System (OPFS) and byte array import/export hooks in `crates/tapirus-wasm`.
- **Reactive Live Queries & Change Data Capture (CDC)**:
  - Implemented in-memory `RealtimeBus` emitting atomic `ChangeEvent` records on table mutations (`conn.subscribe(table, callback)`).
- **System One Model Bridge & Speculative Fan-Out**: Integrated high-speed speculative inference engine for TypeSafe AI with atomic pre-commit invariants and Shannon entropy probability calibration ($H(P) = -\sum P_i \log_2 P_i$).
- **Smart Home Bilingual NLU**: Added intelligent natural language intent parser supporting bilingual Malay and English voice commands (lighting, garage doors, thermostats, air conditioning, and multi-room automation).
- **Relational SQL Enhancements**:
  - Implemented `ConnectionPool` with configurable max connections, idle timeouts, and thread-safe checkout/checkin semantics.
  - Implemented `SELECT DISTINCT` with deduplicated projection evaluation.
  - Implemented `LEFT OUTER JOIN` supporting nullable non-matching rows alongside existing `INNER JOIN`.
- **Tapisaurus Distributed Cluster Blueprint**: Published comprehensive architectural specification (`docs/TAPISAURUS_DISTRIBUTED_BLUEPRINT.md`) establishing the dual-tier continuum between TapirusDB embedded micro-shards and Tapisaurus planet-scale Multi-Raft clusters.
- **Formal Verification Specs**: Integrated TLA+ mathematical models in `docs/formal_verification/` verifying crash-recovery invariants and torn-page defenses under sudden power loss.
- **Enterprise Mission Showcases**: Added aerospace telemetry flight recorder and fintech double-entry ledger verification suites under `examples/system_one/`.

### Changed
- Refactored internal SQL execution pipeline to streamline multi-table join plan optimization.
- Hardened test runner with unified test suites passing across all 12 core modules.

### Fixed
- Fixed command dispatch parsing to execute actions when pressing Enter in interactive web demos.
- Fixed column name resolution in nested projection queries with aggregate expressions.

---

## [0.1.1] - 2026-09-18

### Added
- **Native ChaCha20-Poly1305 AEAD Encryption**:
  - Transparent authenticated page-level encryption at rest with SHA-256 key derivation.
  - 16-byte Key Check Value (KCV) verification preventing unauthorized or corrupted database unlocks.
  - Zero performance overhead for unencrypted databases.
- **Anthropic Model Context Protocol (MCP) Server**:
  - Out-of-the-box stdio JSON-RPC 2.0 server executable via `tapirus mcp [file]`.
  - Seamless memory grounding and SQL execution tools for Claude Desktop, Cursor, and Gemini.
- **Universal Multi-Language Bindings**:
  - C ABI dynamic/static library crate (`crates/tapirus-ffi`).
  - WebAssembly browser and edge runtime crate (`crates/tapirus-wasm`).
  - Python wheel packaging infrastructure (`python/`).
- **GraphRAG Temporal Memory Engine**:
  - Exponential temporal decay function ($e^{-\lambda \Delta t}$) for episodic AI memory.
  - Isolated agent namespaces with hybrid BM25 lexical and HNSW cosine vector search.

### Security
- Added automated memory zeroization on drop for cryptographic key handles.

---

## [0.1.0] - 2026-09-16

### Added
- **Initial Public Release of TapirusDB**:
  - **100% Safe Rust Core**: Compiled under `#![forbid(unsafe_code)]` with zero unsafe blocks and zero C memory corruption risks.
  - **Four-in-One Multi-Model Storage**: Unified Relational SQL-92, JSON Document collections, HNSW Vector search, and Knowledge Graph (GraphRAG) into a single `.tapir` disk file.
  - **ACID Slotted-Page Engine**: Dual-paged B+Tree index with 1–9B Varints, 4KB page size, and Write-Ahead Logging (WAL) with CRC32 integrity checksums.
  - **Interactive CLI & REPL**: Terminal interface with syntax-highlighted tables, query planner explanations, and vector similarity inspections.
  - **Ultra-Compact Footprint**: Less than 4 MB idle RAM footprint with cold database initialization under 500 nanoseconds.
