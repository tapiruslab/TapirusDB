# TapirusDB

**The 100% Safe-Rust (`#![forbid(unsafe_code)]`) Embedded Quad-Model AI Database & Cognitive Memory Engine.**

Relational SQL, HNSW Vector Search, Property Graphs / GraphRAG, JSON Document Collections, and Transactional AI Memory unified inside a single encrypted, crash-safe `.tapir` file and write-ahead log.

[![Crates.io](https://img.shields.io/crates/v/tapirus.svg?style=flat-square&logo=rust)](https://crates.io/crates/tapirus)
[![Core lint](https://img.shields.io/badge/core-unsafe_code_forbidden-brightgreen.svg?style=flat-square)](src/lib.rs)
[![MSRV: Rust 1.88+](https://img.shields.io/badge/rust-1.88%2B-orange.svg?style=flat-square)](Cargo.toml)
[![TLA+ TLC](https://img.shields.io/badge/TLA%2B%20TLC-552k%20states%20verified-blue.svg?style=flat-square)](docs/formal_verification/README.md)
[![License: BUSL-1.1](https://img.shields.io/badge/license-BUSL--1.1-purple.svg?style=flat-square)](LICENSE)

Explore the [web tutorials](https://tapirusdb.com/tutorial), browse the [API cookbook](docs/COOKBOOK.md), or launch the [Rust Browser WASM Workspace](https://tapirusdb.com/studio/).

---

## Core Architecture & Capabilities

- **Four Data Models + AI Memory in One ACID Engine**
  - **Relational SQL & SQLite Compatibility:** `CREATE TABLE`, `CREATE INDEX` (single & multi-column composite indexes), `FOREIGN KEY` (`ON DELETE` / `ON UPDATE`), `CHECK`, `DEFAULT`, `CREATE TRIGGER` (`BEFORE`/`AFTER`, `WHEN`, `RAISE`), `FTS5` full-text search, `WITH` CTEs, subqueries, `ON CONFLICT` upserts, `RETURNING`, window functions, parameterized prepared statements, and C-ABI `sqlite3_*` compatibility (`sqlite3_open_v2`, `sqlite3_prepare_v2`, `sqlite3_bind_*`, `sqlite3_step`, `sqlite3_column_*`, `sqlite3_exec`) in `tapirus-ffi`.
  - **Vector Search & Quantization:** Exact and HNSW approximate nearest neighbors, disk-backed paged HNSW, optional `wide` SIMD kernels (`--features simd`), 8-bit scalar quantization (`QuantizedVector8`), and RaBit bit-packed Walsh-Hadamard quantization (`RaBitQuantizer`).
  - **Graph & GraphRAG:** Labelled property nodes and weighted edges, Compressed Sparse Row (`CsrGraph`) export, PageRank, Dijkstra shortest path, connected components, betweenness centrality, Louvain community detection, `GRAPH MATCH` / `CYPHER` queries, and fluent `GraphChain` hybrid traversal.
  - **JSON Document Collections & Transactional AI Memory:** Schemaless document APIs alongside indexed `storeMemory` / `upsertMemory` / `searchMemories` hybrid retrieval combining HNSW cosine similarity, BM25 lexical scoring, temporal recency decay, and importance via Reciprocal Rank Fusion (RRF) or weighted scoring — persisted in compact binary `TPMEM001` rows with CRC32-validated `__sys_memory_index` HNSW snapshots.
- **MVCC Snapshot Readers & Multi-Process SWMR WAL**
  - `SessionDatabase` provides owned logical write sessions (`session()`, `session_with_timeout()`) plus non-blocking MVCC committed `SnapshotSession` readers (`snapshot_reader()`, `mvcc_epoch()`) that execute immediately without waiting on active uncommitted write transactions.
  - `Connection::open_concurrent*` and `Connection::open_read_only*` coordinate multiple OS processes over SWMR WAL (`refresh_from_disk()`, acquiring `.tapir-lock` only for writers).
- **Hardened Cryptography, Zeroization & Native TLS**
  - **Storage AEAD:** Per-page 256-bit XChaCha20-Poly1305 encryption (`24`-byte CSPRNG `XNonce`) with PBKDF2-HMAC-SHA256 key derivation (`600,000` iterations), constant-time KCV check, and Poly1305 AAD binding `page_id` + Page 1 immutable security/format header fields.
  - **Memory Hygiene:** Non-`Clone` `DatabaseCipher` (`Zeroize` + `Drop`), PBKDF2/HMAC stack buffer zeroization, and `Config` secret scrubbing with `[REDACTED]` `Debug` formatting on `Connection::open*` and `tapirus serve`.
  - **Zero-Trust HTTP & MCP Server (`tapirus serve` / `tapirus mcp`):** Native Rustls TLS 1.2/1.3 (`--tls-cert` / `--tls-key`), loopback-by-default bind, constant-time header-only API key verification, AST-enforced SQL RBAC (`--api-key-role KEY=readonly|readwrite[:tables]`), per-owner AI Memory namespace isolation (`--api-key-namespace`, `MemoryAccessScope`, `Connection::scoped_memory`), opt-in CORS allowlist, defensive HTTP headers, and configurable Ollama / OpenAI-compatible HTTP neural embeddings.
  - **Browser / WASM Encryption:** `@tapirus/wasm` supports end-to-end encrypted in-memory databases and `.tapir` snapshots via `TapirusWasm.new_encrypted(passphrase)` and `TapirusWasm.import_encrypted_bytes(bytes, passphrase)`.
- **Formal Verification & Supply-Chain Discipline**
  - MSRV **Rust 1.88+** tracking patched cryptographic crates (`chacha20poly1305 0.11.0`, `ctutils 0.4.8`, `sha2 0.11.1`), verified via OSV (`scripts/check-dependency-advisories.py`: `0` active production advisories across `139` locked packages).
  - Single-writer WAL crash-safety specification ([TAPIRUS_WAL.tla](docs/formal_verification/TAPIRUS_WAL.tla)) verified in CI by **TLC2** (`552,520` generated states, `120,535` distinct states, `0` invariant violations across `TypeOK`, `RecoveredDiskMatchesCommitted`, `CheckpointedDiskMatchesCommitted`, and `NoUncommittedCheckpoint`).
- **Modular Editions & Edge-to-Cloud Footprint**
  - Build the full Quad engine (`tapirus`), single-model entry points (`tapirus-lite`, `tapirus-document`, `tapirus-graph`, `tapirus-vector`), or bare-metal `no_std` allocation-free flash storage (`tapirus-micro`) with signed offline capsule bridging into Quad (`tapirus-iot`).

---

## Quickstart

### Rust

Add `tapirus` (`rust-version = "1.88"`) to `Cargo.toml`:

```rust
use tapirus::{Config, Connection, MemoryDocument, MemorySearch, Result, SessionDatabase, Value};

fn main() -> Result<()> {
    let db = SessionDatabase::open_with_config(
        "app.tapir",
        Config {
            encryption_key: Some("correct-horse-battery-staple".into()),
            concurrent_access: true,
            ..Default::default()
        },
    )?;

    let writer = db.session()?;
    writer.transaction(|tx| {
        tx.execute(
            "CREATE TABLE IF NOT EXISTS notes (
                id INTEGER PRIMARY KEY,
                body TEXT NOT NULL,
                tag TEXT DEFAULT 'general'
            )",
        )?;
        tx.execute_with_params(
            "INSERT INTO notes (id, body) VALUES (?, ?) ON CONFLICT DO UPDATE SET body = EXCLUDED.body",
            &[Value::Integer(1), Value::Text("Encrypted WAL + SQL + AI memory".into())],
        )?;
        tx.store_memory(&MemoryDocument {
            content: "Engine oil maintenance is due every 500 operating hours.".into(),
            namespace: Some("manuals".into()),
            ..Default::default()
        })?;
        Ok(())
    })?;

    // Non-blocking MVCC committed snapshot reader
    let reader = db.snapshot_reader()?;
    let rows = reader.query("SELECT id, body, tag FROM notes")?;
    println!("{rows:?}");

    let hits = writer.search_memories(&MemorySearch {
        query: Some("maintenance interval".into()),
        namespace: Some("manuals".into()),
        limit: 5,
        ..Default::default()
    })?;
    println!("Top hit: {:?}", hits.first().map(|h| &h.entry.content));

    writer.close()?;
    db.close()?;
    Ok(())
}
```

Or build single-model binaries cleanly from source:

```bash
cargo build --release -p tapirus --no-default-features --features lite --bin tapirus-lite
cargo build --release -p tapirus --no-default-features --features document --bin tapirus-document
cargo build --release -p tapirus --no-default-features --features graph-only --bin tapirus-graph
cargo build --release -p tapirus --no-default-features --features vector-only --bin tapirus-vector
```

### Node.js (`sdks/nodejs`)

```js
const { open } = require('tapirus');

const db = open('assistant.tapir');
db.storeMemory({
  content: 'Maintenance is due every 500 hours.',
  namespace: 'manuals',
  tags: ['source:service-guide'],
});
const sources = db.searchMemories({
  query: 'maintenance interval',
  namespace: 'manuals',
  limit: 5,
});
console.log(sources.map(s => ({ id: s.entry.id, text: s.entry.content, score: s.combined_score })));
db.close();
```

### Python (`sdks/python`)

```python
import tapirus

with tapirus.connect("assistant.tapir", "strong-passphrase") as db:
    db.execute("CREATE TABLE IF NOT EXISTS events (id INTEGER PRIMARY KEY, label TEXT NOT NULL)")
    db.execute("INSERT INTO events VALUES (?, ?)", [1, "startup"])
    db.store_memory({
        "content": "Replace intake filter at 500 operating hours.",
        "namespace": "manuals",
    })
    print(db.search_memories({"query": "intake filter", "namespace": "manuals", "limit": 5}))
```

### Secure HTTP & MCP Daemon (`tapirus serve` / `tapirus mcp`)

Start `tapirus serve` with native Rustls TLS 1.2/1.3, namespace-isolated API keys, AST-enforced SQL RBAC, and explicit CORS allowlisting:

```bash
tapirus serve assistant.tapir \
  --bind 0.0.0.0:3443 \
  --tls-cert cert.pem \
  --tls-key key.pem \
  --api-key "$ADMIN_KEY" \
  --api-key-role "$READER_KEY=readonly:notes" \
  --api-key-namespace "$AGENT_KEY=workspace-alpha" \
  --cors-origin https://app.example.com
```

Run an MCP server backed by a local Ollama or OpenAI-compatible neural embedding endpoint:

```bash
tapirus mcp assistant.tapir \
  --embed-endpoint http://127.0.0.1:11434/api/embed \
  --embed-model nomic-embed-text \
  --embed-dims 768 \
  --namespace agent-memory
```

---

## Build, Verification & Packaging

Verify formatting, strict lints, standalone editions, dependency advisories, and the full test suite locally:

```bash
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets --all-features --offline -- -D warnings
cargo test --locked --workspace --offline
python scripts/check-dependency-advisories.py
```

Containerized execution (runs as a non-root user with `/data` volume persistence):

```bash
docker build -t tapirusdb:local .
docker run --rm --network none --mount source=tapirus-demo,target=/data \
  tapirusdb:local /data/demo.tapir -c "CREATE TABLE notes (id INTEGER PRIMARY KEY, body TEXT)"
```

---

## Documentation Map

- **Security & Formal Verification**
  - [SECURITY.md](SECURITY.md) — Vulnerability disclosure, cryptographic guarantees, and deployment hardening
  - [Security Architecture & Threat Model](docs/security/SECURITY_ARCHITECTURE_AND_THREAT_MODEL.md) — Trust boundaries and third-party audit readiness dossier
  - [TLA+ Formal WAL Specification](docs/formal_verification/README.md) — Crash-safety model and CI TLC2 verification
  - [Engineering Assurance Architecture](docs/HIGH_ASSURANCE_SAFETY.md) — Safety boundaries, memory safety, and verification controls
- **Architecture & Guides**
  - [API Cookbook](docs/COOKBOOK.md) — Verified Rust, SQL, Vector, Graph, Encryption, and AI Memory recipes
  - [Transactional AI Memory](docs/AI_MEMORY.md) & [AI Embedded Workflows](docs/AI_EMBEDDED_WORKFLOWS.md)
  - [Editions & Build Profiles](docs/EDITIONS.md) | [IoT Capsules (`tapirus-micro`)](docs/IOT_CAPSULE.md)
  - [Storage & Paging](docs/architecture/01_STORAGE_ENGINE_AND_PAGING.md) | [WAL, MVCC & Recovery](docs/architecture/02_WAL_TRANSACTIONS_AND_RECOVERY.md) | [SIMD Vector & Quantization](docs/architecture/03_SIMD_VECTOR_AND_QUANTIZATION.md) | [GraphBLAS & openCypher](docs/architecture/04_GRAPHBLAS_AND_OPENCYPHER.md)
  - [Language SDKs](sdks/README.md) | [Release Validation](docs/RELEASE_VALIDATION.md) | [Current Release Status](docs/CURRENT_STATUS.md)
  - [Panduan TAP (BM)](docs/TAP_GUIDE.md) | [Interactive Local Web Guide (BM)](docs/tapirusdb-guide.html)
