# TapirusDB: Architecture & Design Overview for an Embedded Quad-Model AI Database

## Abstract

TapirusDB is a Safe-Rust (`#![forbid(unsafe_code)]`) embedded database that unifies **Relational SQL (with SQLite C-ABI compatibility)**, **HNSW Vector Search & Quantization**, **Property Graphs / GraphRAG**, **JSON Document Collections**, and **Transactional AI Document Memory** inside a single `.tapir` storage file paired with a Single-Writer Multiple-Reader (SWMR) Write-Ahead Log and non-blocking MVCC committed snapshot readers.

## 1. Unified Multi-Model Core

- **Relational SQL & Constraints:** Slotted-page B+Trees back SQL tables, single- and multi-column composite indexes, `FOREIGN KEY` (`ON DELETE` / `ON UPDATE`), `CHECK`, `DEFAULT`, `CREATE TRIGGER`, `FTS5` full-text search, `WITH` CTEs, subqueries, `ON CONFLICT` upserts, `RETURNING`, and window functions, exposed in Rust and through C-ABI `sqlite3_*` bindings in `tapirus-ffi`.
- **Single-File `.tapir` Atomicity:** Relational tables, vector indexes, graph topologies, JSON collections, and AI memory entries (`TPMEM001` + `__sys_memory_index` HNSW snapshots) participate in the same transactional WAL commit, rollback, savepoint, and crash-recovery lifecycle.

## 2. Storage, MVCC Snapshots & Crash Recovery

- **4 KiB Paged Engine & LZ4 Compression:** The pager manages slotted B+Tree pages, overflow chains, freelist reclamation, and optional Safe-Rust LZ4 frame compression (`src/pager/compression.rs`).
- **MVCC Non-Blocking Readers & SWMR WAL:** `SessionDatabase::snapshot_reader()` serves committed read epochs lock-free alongside active writers, while `Connection::open_concurrent*` coordinates multi-process SWMR writes via `.tapir-lock` and `refresh_from_disk()`.
- **Formal WAL Model Verification:** Single-writer WAL transitions, checkpoints, crashes, and recovery are formally modeled in [TAPIRUS_WAL.tla](docs/formal_verification/TAPIRUS_WAL.tla) and checked exhaustively by **TLC2** in CI (`552,520` generated states, `120,535` distinct states, `0` invariant violations).

## 3. Vector Representation & Hybrid AI Retrieval

- **HNSW & Quantization:** Supports exact scan, HNSW graph search (`ef_construction = 128`, `ef_search = 512` default), paged disk-backed HNSW, 8-bit scalar quantization (`QuantizedVector8`), and bit-packed Walsh-Hadamard quantization (`RaBitQuantizer`), with optional `wide` SIMD vector kernels.
- **Hybrid Fusion & Scoped Memory:** AI Memory combines cosine similarity, BM25 lexical scoring, temporal recency decay, and importance via Reciprocal Rank Fusion (`rrf_k = 20.0`), enforced within hard multi-tenant boundaries (`MemoryAccessScope`, `Connection::scoped_memory`).

## 4. Security & Zero-Trust Transport

- **Per-Page XChaCha20-Poly1305 AEAD:** 256-bit keys (PBKDF2-HMAC-SHA256, 600k iterations), 24-byte CSPRNG `XNonce`, constant-time KCV, and Page 1 header AEAD AAD authentication.
- **Zeroization & Secret Scrubbing:** Non-`Clone` `DatabaseCipher` (`Zeroize` + `Drop`), PBKDF2/HMAC working buffer zeroization, and `Config` credential redaction/scrubbing on open.
- **Native Rustls TLS & AST RBAC:** `tapirus serve` enforces native TLS 1.2/1.3 (`rustls` + `ring`), AST-level per-table SQL RBAC (`--api-key-role`), namespace-scoped AI memory keys (`--api-key-namespace`), and opt-in CORS allowlisting.
- **Browser / WASM Vaults:** `@tapirus/wasm` supports `TapirusWasm.new_encrypted(passphrase)` and `import_encrypted_bytes` for end-to-end encrypted in-browser `.tapir` execution and export.

## 5. Benchmark Reproducibility & Validation

Scoped SQL and HNSW measurements (`cargo bench --bench release_readiness --offline`) record warm point reads at `4.9 / 14.6 / 18.6 us` (`p50 / p95 / p99`) and HNSW nearest-neighbor recall at `99.9%` (`81.9 us` `p50`) alongside SQLite `3.46.0` WAL parity checks, hardware metadata, and `Cargo.lock` hashes. See [Release Validation](docs/RELEASE_VALIDATION.md) for the complete verification matrix.

## Further Reading

- [API Cookbook](docs/COOKBOOK.md)
- [Storage Engine & Paging](docs/architecture/01_STORAGE_ENGINE_AND_PAGING.md)
- [WAL, MVCC & Recovery](docs/architecture/02_WAL_TRANSACTIONS_AND_RECOVERY.md)
- [Engineering Assurance Architecture](docs/HIGH_ASSURANCE_SAFETY.md)
- [Security Architecture & Threat Model](docs/security/SECURITY_ARCHITECTURE_AND_THREAT_MODEL.md)
