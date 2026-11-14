# TapirusDB Engineering Roadmap & Delivered Milestones

This document tracks delivered platform milestones in `1.0.5+` and active engineering initiatives on [main](https://github.com/tapiruslab/TapirusDB/tree/main).

---

## Delivered Platform Milestones (`1.0.4 – 1.0.5+`)

- **Persisted Binary AI Memory & Instant HNSW Reload:** Memory documents store in compact bounds-checked binary rows (`TPMEM001` in `__sys_memory`) paired with CRC32-validated persisted HNSW index snapshots (`__sys_memory_index`).
- **Tuned Hybrid Retrieval & External Neural Embeddings:** `MemoryConfig::default()` (`ef_construction = 128`, `ef_search = 512`) and `MemorySearch::default()` (`rrf_k = 20.0`, `vector_weight = 0.65`, `lexical_weight = 0.25`) optimize top-K and paraphrase recall, backed by Ollama (`/api/embed`) and OpenAI-compatible (`/v1/embeddings`) neural endpoints in `tapirus mcp` plus local E5 ONNX inference in `tapirus-agent`.
- **Multi-Tenant Memory Access Isolation:** Hard `MemoryAccessScope` enforcement via `Connection::scoped_memory`, `tapirus serve --api-key-namespace`, and `tapirus mcp --namespace`.
- **Expanded SQLite Core & C-ABI Completeness:** Added `FOREIGN KEY` (`ON DELETE`/`ON UPDATE`), `CHECK`, `DEFAULT`, `CREATE TRIGGER` (`BEFORE`/`AFTER`, `WHEN`, `RAISE`), multi-column composite indexes, `FTS5` virtual tables, and `tapirus-ffi` `sqlite3_bind_*` / `sqlite3_exec` / deferred `sqlite3_prepare_v2`.
- **MVCC Snapshot Readers & Multi-Process SWMR WAL:** Added lock-free committed `SessionDatabase::snapshot_reader()` (`mvcc_epoch()`) alongside multi-process `.tapir-lock` SWMR coordination (`Config::concurrent_access`, `refresh_from_disk()`).
- **Zero-Trust Security & Cryptography Stack:** Delivered native Rustls TLS 1.2/1.3, AST-enforced SQL RBAC (`--api-key-role`), Page 1 AEAD AAD header authentication, non-`Clone` `Zeroize` `DatabaseCipher`, `Config` secret scrubbing, WASM `TapirusWasm::new_encrypted`, and Rust `1.88+` MSRV tracking patched `chacha20poly1305 0.11.0`, `ctutils 0.4.8`, and `sha2 0.11.1` (0 production OSV advisories).
- **Formal Verification in CI:** `docs/formal_verification/TAPIRUS_WAL.tla` executes live **TLC2** checks on every push (`552,520` states generated, `120,535` distinct states, `0` violations).

---

## Active Engineering Initiatives

1. **External Independent Cryptographic Audit:** Engage an independent security firm using [Security Architecture & Threat Model](docs/security/SECURITY_ARCHITECTURE_AND_THREAT_MODEL.md) as the audit scope specification and publish the signed third-party report alongside the corresponding release tag.
2. **Large-Scale Paged DiskANN / HNSW Streaming:** Extend `src/vector/paged_hnsw.rs` and `MemoryEngine` for multi-million-document out-of-core graph traversal under strict RAM budgets.
3. **Multi-Language ORM Driver Validation:** Expand automated compatibility suites for Prisma, SQLAlchemy, and Knex against `tapirus-ffi`'s `sqlite3_*` C ABI.
4. **Hardware-Qualified `tapirus-micro` Board Drivers:** Deliver production flash drivers and physical power-cut/wear qualification on named Cortex-M and RISC-V targets.
5. **Expanded Public Retrieval & Multi-Scale Benchmarks:** Publish multi-hardware benchmark baselines across BEIR/MTEB corpora and large OLTP/HTAP workloads with full reproducibility artifacts.
