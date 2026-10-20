# TapirusDB

TapirusDB is an embedded Rust database for local applications that need SQL, vector search and AI document memory in one runtime. Store text and embeddings, retrieve sources for an LLM, and keep SQL and memory changes in the same transaction.

[![Crates.io](https://img.shields.io/crates/v/tapirus.svg?style=flat-square&logo=rust)](https://crates.io/crates/tapirus)
[![Core lint](https://img.shields.io/badge/core-unsafe_code_forbidden-brightgreen.svg?style=flat-square)](src/lib.rs)
[![License: BUSL-1.1](https://img.shields.io/badge/license-BUSL--1.1-purple.svg?style=flat-square)](LICENSE)

Build local AI products with a data layer that runs inside your application.
Start with the [web tutorials](https://tapirusdb.com/tutorial), explore the
[API cookbook](https://tapirusdb.com/cookbook), or
[try the Rust browser workspace](https://tapirusdb.com/studio/).

The default branch includes storage corrections, transactional AI memory and
native SDK integration coverage. Published release archives may predate these
changes; see [release validation](docs/RELEASE_VALIDATION.md) for evidence and
the distinction between current source and published packages.

## Highlights

- Local, in-process database with a `.tapir` file and a write-ahead log.
- SQL support is a documented subset; TapirusDB is not a SQLite drop-in or a claim of full SQL conformance.
- Vector, graph, and JSON document APIs are available alongside relational tables.
- AI source retrieval combines BM25, supplied embeddings, recency and importance, with namespace/session filters and source citations.
- Indexed memory adds model-specific HNSW, upsert, revision checks, atomic batches, source versions, TTL and configurable budgets. Python tools provide verified SQLite scalar migration, document ingestion and retrieval evaluation. See [AI and embedded workflows](docs/AI_EMBEDDED_WORKFLOWS.md).
- `SessionDatabase` provides independent transaction ownership over serialized access. The separate [Micro component](crates/tapirus-micro/README.md) targets bounded bare-metal flash records.
- The Rust WASM workspace opens and exports native `.tapir` files and saves committed browser snapshots to IndexedDB.
- First-party SQL Lite, Document, Graph and Vector entry points can be built separately; the default Quad engine exposes every model in the same native file. See [edition builds and limits](docs/EDITIONS.md).
- Page encryption is optional. Review key handling and deployment configuration for your threat model.
- The main `tapirus` crate forbids explicit unsafe Rust. The C ABI crate contains unsafe boundary code. This does not prove functional correctness, crash safety, or bounded resource use.

## Rust quickstart

Need only one model? Build `tapirus-lite`, `tapirus-document`, `tapirus-graph`,
or `tapirus-vector` from the current source. Quad remains the default `tapirus`
binary. For example, a SQL-only CLI build is:

```bash
cargo build --release -p tapirus --no-default-features --features lite --bin tapirus-lite
```

The edition binaries share the `.tapir` format, but they are API boundaries,
not security sandboxes or proven low-memory builds. Published v1.0.2 artifacts
may not include these new binaries; check the asset list for the exact release.
Follow the [choose-an-edition tutorial](docs/tutorials/choose_an_edition.md)
for a file-interchange walkthrough.

Add the crate to a Rust project, then open a database and use the SQL API:

```rust
use tapirus::{Connection, Result};

fn main() -> Result<()> {
    let db = Connection::open("notes.tapir")?;
    db.execute("CREATE TABLE IF NOT EXISTS notes (id INTEGER PRIMARY KEY, body TEXT NOT NULL)")?;
    db.execute("INSERT INTO notes (id, body) VALUES (1, 'Check the recovery path')")?;

    let rows = db.query("SELECT id, body FROM notes")?;
    println!("{rows:?}");
    db.close()?;
    Ok(())
}
```

Use parameterized APIs for user-provided values. Review the SQL parser and executor for the supported syntax and constraints before depending on a query shape.

## Architecture and workload fit

TapirusDB is intended for local, single-process workloads that benefit from colocated relational, vector, graph, or document data. Its API and query support should be evaluated against the application requirements.

`ConnectionPool::acquire()` currently returns a clone of the same connection. Handles share session and transaction state; this is not a pool of independent database sessions. SQL execution uses shared locks, so the pool name does not imply concurrent SQL readers.

Use `SessionDatabase` when transactions need independent owners. Other sessions
return Busy or wait until the owning session finishes; access remains serialized.

WAL recovery, replication, backups, vector snapshots, HTTP behavior, and SDK bindings have regression tests for selected cases. Tests cover only the scenarios they exercise. Validate crash recovery, resource use, backup and restore, and SDK behavior on the target platform before storing important data.

## AI document memory

```js
const { open } = require('tapirus');
const db = open('assistant.tapir');
db.storeMemory({ content: 'Maintenance is due every 500 hours.', namespace: 'manuals' });
const sources = db.searchMemories({ query: 'maintenance interval', namespace: 'manuals', limit: 5 });
console.log(sources.map(source => ({ id: source.entry.id, text: source.entry.content })));
db.close();
```

Supply an `embedding` from your chosen model to add vector similarity. These methods run locally and participate in SQL transactions. See [the AI memory guide](docs/AI_MEMORY.md) for Rust, Node.js, score definitions and browser use.

## HTTP server

`tapirus serve` exposes HTTP endpoints. Set an API key when the server is reachable by untrusted clients, bind to an appropriate interface, and use the documented request and response shapes. The browser chat page is public; chat and chat-log API routes require the configured key.

## Security, performance, and formal methods

- See [SECURITY.md](SECURITY.md) for the threat boundary, optional encryption, and reporting guidance.
- Historical latency, throughput, RAM, and binary-size figures are withdrawn pending corrected, reproducible measurements. See [Benchmark status](#benchmark-status).
- TLA+ files describe selected WAL transitions. The repository has no TLC run output or refinement proof connecting the model to Rust; see [the model scope](#tla-model-scope).
- No ISO 26262, IEC 62304, DO-178C, or other functional-safety certification is claimed.

## Build and test

```bash
cargo test --workspace --all-targets --offline
cargo check --workspace --all-features --offline
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --offline -- -D warnings
```

These commands validate the current host build and exercised tests. They do not establish formal verification, certification, or correctness for every failure mode.

## Documentation

- [Editions and build commands](docs/EDITIONS.md)
- [Choose an edition tutorial](docs/tutorials/choose_an_edition.md)
- [API cookbook](docs/COOKBOOK.md)
- [WAL and recovery architecture notes](docs/architecture/02_WAL_TRANSACTIONS_AND_RECOVERY.md)
- [TLA+ model scope](docs/formal_verification/README.md)
- [Assurance scope and limitations](docs/HIGH_ASSURANCE_SAFETY.md)
- [SDKs](sdks/README.md)
- [Transactional AI memory](docs/AI_MEMORY.md)
- [Release validation and packaging](docs/RELEASE_VALIDATION.md)

## Benchmark status

The audit found benchmark cases that need correction or clearer scope, including operations whose results may be optimized away, a dot-product case labeled as cosine, extrapolated vector-memory estimates, and comparisons with different durability settings. Do not use historical performance figures as product claims.

The corrected `cargo bench --bench release_readiness` harness checks results, reopens both databases and measures HNSW recall against exact neighbors. Its small-workload local results and limitations are recorded in [release validation](docs/RELEASE_VALIDATION.md). Publish the source revision, binary hash, compiler flags, hardware, storage settings, dataset, warm or cold state, and latency distribution with every result.

## TLA+ model scope

The repository contains a TLA+ model of selected WAL and recovery transitions. There is no TLC run output or refinement proof connecting the model to the Rust implementation. The model is a design artifact, not evidence that the compiled engine is formally verified.
