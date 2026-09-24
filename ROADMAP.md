# 🗺️ TapirusDB Roadmap

This roadmap outlines the past achievements, current architecture, and upcoming milestones for **TapirusDB**.

---

## 📍 Completed Milestones

### ✅ v0.1.0 — The Embedded Multi-Model Foundation
- [x] Pure Safe-Rust engine core (`#![forbid(unsafe_code)]`).
- [x] Slotted-page B+Tree storage engine with 4KB pages and 1–9B Varints.
- [x] Multi-model unification in single `.tapir` file (Relational, Documents, Vectors, Graph).
- [x] Write-Ahead Logging (WAL) with CRC32 frame checksums.
- [x] Terminal CLI and REPL with syntax highlighting.

### ✅ v0.1.1 — Encryption, MCP & Universal Bindings
- [x] Authenticated encryption at rest (ChaCha20-Poly1305 AEAD) with SHA-256 KDF and KCV verification.
- [x] Native Anthropic Model Context Protocol (MCP) server stdio integration.
- [x] GraphRAG temporal memory decay ($e^{-\lambda \Delta t}$) and BM25 hybrid ranking.
- [x] Universal C-FFI (`crates/tapirus-ffi`) and WebAssembly runtime (`crates/tapirus-wasm`).

### ✅ v0.1.2 — Next-Gen Pillars: Enterprise GraphRAG, Serverless S3/R2 & Ecosystem
- [x] **Accelerated GraphRAG Suite**: High-speed Product Quantization (PQ) seed discovery, micro-hop graph adjacency traversal ($O(1)$ per edge), and Tri-Modal Reciprocal Rank Fusion (RRF) combining vector, lexical, and structural graph proximity with prompt-ready context synthesis.
- [x] **Product Quantization (PQ)**: In-memory `ProductQuantizer` delivering 16x–32x RAM reduction with fast Asymmetric Distance Computation (ADC).
- [x] **S3 / Cloudflare R2 Remote Storage Adapter (`RemotePager`)**: Cloud-native 4KB on-demand page streaming via HTTP Range Requests (`bytes=offset-(offset+4095)`) with in-memory page caching.
- [x] **Cloudflare Worker Serverless Edge Adapter**: Ultra-fast V8 isolate adapter (`examples/cloudflare-worker/`) with sub-5ms cold starts for SQL, GraphRAG, and AI Agent Memory.
- [x] **Official Package Ecosystem**: Published official NPM (`@tapirus/db`), Python PyPI wheel configs, macOS/Linux Homebrew formula (`Formula/tapirus.rb`), and Windows Winget manifest (`winget/tapirus.yaml`).
- [x] **Reactive Live Queries & Change Data Capture (CDC)**: In-memory `RealtimeBus` and atomic table mutation event listeners (`conn.subscribe(table, callback)`).
- [x] **Hot Online Backup & Time-Travel SQL**: Non-blocking `VACUUM INTO` snapshots and `FOR SYSTEM_TIME AS OF <timestamp>` parser support.
- [x] **Tier B Bare-Metal Embedded Silicon & MCU Support**: Zero-OS `FlashBlockDevice` and `MicroPager` (512B micro-blocks) driver for ESP32/STM32/RISC-V with sub-512KB SRAM, enabling deterministic telemetry logging, 8-bit quantized micro-vectors, and micro-graph edge traversal.
- [x] **Robotics & Autonomous Automotive Intelligence Guide**: Complete 3-Tier Hardware Continuum architecture (`docs/ROBOTICS_AUTOMOTIVE_EDGE.md`) featuring ROS2 C++ perception integration and ISO 26262 / ASIL-D safety readiness architecture.
- [x] **System One Speculative Fan-Out Engine**: Shannon entropy probability calibration ($H(P) = -\sum P_i \log_2 P_i$) and bilingual Malay/English smart home NLU.
- [x] **Relational SQL Extensions**: Multi-threaded `ConnectionPool`, `SELECT DISTINCT`, and `LEFT OUTER JOIN` evaluation.
- [x] **TLA+ Formal Verification**: Mathematical models verifying atomic WAL recovery and torn-page defenses.

---

## 🚀 Near-Term Milestones (v0.2.0)

- [ ] **Declarative Graph Cypher/GQL Query Dialect**: Direct `MATCH (a)-[r:KNOWS]->(b)` syntax in SQL parser.
- [ ] **SIMD Hardware Acceleration (AVX-512 & ARM NEON)**: Vectorized dot products and quantized ADC routines for hardware silicon.
- [ ] **Automated CDC Webhook Dispatcher**: Direct streaming transaction webhook dispatcher for Kafka and RabbitMQ pipelines.
- [ ] **Dual-Engine Full-Text Search (FTS5)**: Advanced linguistic stemming and phonetic matching on document text.

---

## 🌐 Strategic Horizon (v0.3.0+) — Tapisaurus Planetary Cloud Mesh

- [ ] **Tapisaurus Replication Gateway**:
  - Two-way delta synchronization bridge between local `.tapir` embedded micro-shards and distributed **Tapisaurus** cloud clusters.
  - Conflict-Free Replicated Data Types (CRDTs) for offline-first edge applications.
- [ ] **Multi-Raft Planet-Scale Clustering** (under the Tapisaurus distributed engine):
  - Consensus-driven geo-distributed sharding with wire compatibility for PostgreSQL, MongoDB, and MySQL gateways.
  - Global query routing coordinator with 2-tier hierarchical vector centroids and federated graph edge traversal.
