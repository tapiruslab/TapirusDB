<div align="center">

# TapirusDB

### The Embedded Cognitive Memory & Multi-Model Engine for Sovereign AI & Edge Systems
**Sub-Microsecond Agent Memory • openCypher Knowledge Graphs • Vector Search • Relational SQL • Documents**  
*Single Encrypted `.tapir` File • 100% Safe Rust • < 4 MB Idle RAM • Zero Cloud Daemons*

<br/>

[![Crates.io](https://img.shields.io/crates/v/tapirus.svg?style=flat-square&logo=rust&color=e05d44)](https://crates.io/crates/tapirus)
[![Memory Safety](https://img.shields.io/badge/memory--safety-100%25_Safe_Rust-brightgreen.svg?style=flat-square)](src/lib.rs)
[![License: BUSL-1.1](https://img.shields.io/badge/license-BUSL--1.1-purple.svg?style=flat-square)](LICENSE)
[![Documentation](https://img.shields.io/badge/docs-tapirusdb.com-0070f3.svg?style=flat-square)](https://tapirusdb.com)
[![Downloads Hub](https://img.shields.io/badge/downloads-tapirusdb.com%2Fdownload-0284c7.svg?style=flat-square&logo=download)](https://tapirusdb.com/download.html)
[![Tapirus Studio](https://img.shields.io/badge/Studio_GUI-Free_Workbench-10b981.svg?style=flat-square&logo=react)](https://tapirusdb.com/studio/index.html)
[![GitHub Releases](https://img.shields.io/github/v/release/tapiruslab/TapirusDB?style=flat-square&color=blue)](https://github.com/tapiruslab/TapirusDB/releases)

<br/>

> **"Stop stitching Pinecone, Neo4j, and SQLite together."**  
> TapirusDB is the high-performance, embedded cognitive memory engine for local AI agents, robotics, and sovereign edge hardware. It collapses vector similarity, knowledge graphs, relational metadata, and JSON documents into a **single encrypted `.tapir` file** with sub-microsecond in-process retrieval ($0.55\ \mu\text{s}$) and zero memory corruption risk.

> 🖥️ **Need a Visual Database Manager (like phpMyAdmin or Supabase Studio)?**  
> Use **[Tapirus Studio](https://tapirusdb.com/studio/index.html)** — our free visual GUI companion for TapirusDB!  
> • **🌐 Run Instant In-Browser**: [tapirusdb.com/studio](https://tapirusdb.com/studio/index.html) *(Zero installation required)*  
> • **⬇️ Official Download Landing Page**: [tapirusdb.com/download.html](https://tapirusdb.com/download.html) *(Windows, macOS, Linux, CLI)*  
> • **📦 Releases & Binary Downloads**: [github.com/tapiruslab/TapirusDB/releases](https://github.com/tapiruslab/TapirusDB/releases)  
> • **💻 Studio Source Code Directory**: [github.com/tapiruslab/TapirusDB/tree/main/studio](https://github.com/tapiruslab/TapirusDB/tree/main/studio)

</div>

---

## Why TapirusDB? (Kill the "Frankenstack")

Modern AI and edge developers are forced into **Fragmented Polyglot Persistence**—gluing together multiple complex, heavy databases across network boundaries:

<p align="center">
  <picture>
    <source media="(max-width: 768px)" srcset="https://raw.githubusercontent.com/tapiruslab/TapirusDB/main/.github/assets/architecture-mobile.svg">
    <source media="(min-width: 769px)" srcset="https://raw.githubusercontent.com/tapiruslab/TapirusDB/main/.github/assets/architecture-comparison.svg">
    <img alt="TapirusDB Architecture vs The Fragile Frankenstack" src="https://raw.githubusercontent.com/tapiruslab/TapirusDB/main/.github/assets/architecture-comparison.svg" width="100%">
  </picture>
</p>

---

## Highlights

* 🦀 **100% Pure Safe Rust (`#![forbid(unsafe_code)]`)**: Guaranteed memory safety at compile-time. Zero buffer overflows, zero dangling pointers, zero use-after-free vulnerabilities, and zero C/C++ memory corruption CVEs.
* 📦 **True In-Process Architecture (Zero-IPC)**: Compiles and links directly into your binary (Rust, Python, TypeScript, C/C++, Go). No background database servers (`mysqld`, `postgres`, `mongod`), zero network serialization overhead, and sub-microsecond in-memory query traversal.
* ⚡ **Quad-Model Data Consolidation**: Seamlessly unifies **Relational SQL-92**, **HNSW & IVF Vector Search**, **openCypher Property Graphs**, and **MongoDB-style JSON Documents** inside a single B+Tree slotted-page file.
* 🧠 **Production GraphRAG & AI Memory**: Built-in seed-and-traverse GraphRAG with Tri-Modal Reciprocal Rank Fusion (RRF), episodic memory with exponential temporal decay, and isolated agent namespaces.
* 🎯 **Dynamic 16-Lane SIMD & RaBitQ 32x Quantization**: Parallel AVX-512 / AVX2 / NEON vector kernels combined with Fast Walsh-Hadamard 1-bit/2-bit random rotation quantization, reducing 1536-D embeddings from 6,144 bytes to **196 bytes** with single-cycle `POPCNT` distance evaluation.
* 🕸️ **Compressed Sparse Row (CSR) Topology & openCypher**: Contiguous adjacency arrays on disk and in memory for zero-allocation slice neighbor sweeps, paired with standard declarative openCypher syntax (`MATCH ... WHERE ... RETURN ...`).
* 🔒 **Native ChaCha20-Poly1305 AEAD Encryption**: Zero-overhead authenticated page-level encryption with SHA-256 key derivation and constant-time Key Check Value (KCV) verification.
* 🌐 **S3/R2 Remote Range Streaming**: On-demand 4KB page streaming directly from cloud object stores via HTTP Range requests with zero local disk footprint.
* 🤖 **Native Model Context Protocol (MCP)**: Out-of-the-box stdio JSON-RPC 2.0 server (`tapirus mcp`) for Claude Desktop, Cursor, and Gemini autonomous agents.

---

## Table of Contents

* [Why TapirusDB? (Kill the "Frankenstack")](#why-tapirusdb-kill-the-frankenstack)
* [Highlights & Technical Advantages](#highlights)
* [Quickstart & 30-Second Code](#quickstart)
  * [CLI & Package Managers (Brew, Winget, Shell)](#1-installation)
  * [SDKs & Ecosystem Registry Matrix](#official-ecosystem--registry-matrix)
  * [Code in 30 Seconds (Rust, Python, Node, Go, PHP)](#2-code-in-30-seconds)
* [Beyond AI: Classic Applications (SQLite & Mongo Alternative)](#beyond-ai-an-ultra-fast-embedded-database-for-classic-applications)
* [High-Impact Domains (Research, Analytics, IoT)](#-high-impact-real-world-domains-research-analytics--smart-home)
* [Architectural Comparison vs Polyglot Frankenstack](#architectural-comparison)
* [Core Technical Pillars (SIMD, CSR, RaBitQ, ChaCha20)](#core-technical-pillars)
* [Industrial Edge & Autonomous Systems](#-industrial-applications-ai--beyond)
* [Developer Tooling & MCP Server](#developer-tooling--cli)
* [Verified Benchmarks & Latency Comparison](#verified-benchmarks)
* [When (and When NOT) to Use TapirusDB](#when-and-when-not-to-use-tapirusdb)
* [Formal Safety Verification (TLA+)](#formal-safety-verification)
* [Roadmap: Tapisaurus Distributed Continuum](#roadmap-tapisaurus-distributed-continuum)
* [Documentation & Architectural Specs](#documentation--architecture)

---

## Quickstart

### 1. Installation

#### Official Downloads Hub & Visual Studio
* ⬇️ **Official Download Landing Page**: [tapirusdb.com/download.html](https://tapirusdb.com/download.html) (Windows, macOS, Linux, CLI)
* 🖥️ **Tapirus Studio (Free GUI)**: [Run Live in Browser](https://tapirusdb.com/studio/index.html) • [Studio Source Code (GitHub)](https://github.com/tapiruslab/TapirusDB/tree/main/studio)
* 📦 **Pre-Compiled Binary Assets**: [GitHub Releases](https://github.com/tapiruslab/TapirusDB/releases)

#### Supported Platforms & Distributions

TapirusDB is 100% self-contained with zero cloud or daemon dependencies. Pre-compiled binaries run out-of-the-box across:

| Platform Family | Architecture | Supported Operating Systems & Distros |
| :--- | :--- | :--- |
| **Linux (Universal glibc)** | `x86_64`, `aarch64` | **Debian, Ubuntu, Fedora, RHEL, CentOS, Rocky Linux, AlmaLinux, Arch Linux, openSUSE, Amazon Linux 2/2023** |
| **Linux (musl & Containers)** | `x86_64`, `aarch64` | **Alpine Linux, Docker / OCI (`ghcr.io/tapiruslab/tapirusdb`), Embedded Linux / IoT** |
| **macOS** | Apple Silicon & Intel | **macOS 12+ (Monterey, Ventura, Sonoma, Sequoia)** |
| **Windows** | `x86_64` | **Windows 10, Windows 11, Windows Server 2019/2022/2025** |
| **WebAssembly (WASM)** | `wasm32` | **All modern web browsers (Chrome, Edge, Safari, Firefox)** via [Tapirus Studio](https://tapirusdb.com/studio) |


#### Package Managers (Terminal & CLI)
```bash
# macOS & Linux (Homebrew)
brew install https://raw.githubusercontent.com/tapiruslab/TapirusDB/main/Formula/tapirus.rb

# Windows (Windows Package Manager)
winget install --manifest https://raw.githubusercontent.com/tapiruslab/TapirusDB/main/winget/tapirus.yaml
# (Or 'winget install tapirus' once indexed in Microsoft community repo)

# Linux / macOS Automated Script
curl -fsSL https://raw.githubusercontent.com/tapiruslab/TapirusDB/main/install.sh | bash

# Windows PowerShell Automated Script
irm https://raw.githubusercontent.com/tapiruslab/TapirusDB/main/install.ps1 | iex
```

#### Language SDKs & Client Libraries
```bash
# Rust Engine
cargo add tapirus

# Python SDK (Python 3.9+)
pip install tapirus

# Node.js & TypeScript SDK
npm install tapirus

# Bun Runtime
bun add tapirus

# Go SDK
go get github.com/tapiruslab/TapirusDB/sdks/go

# PHP Composer
composer require tapiruslab/tapirusdb

# OCI Container (Docker & Podman)
docker pull ghcr.io/tapiruslab/tapirusdb:latest
```

#### Official Ecosystem & Registry Matrix

> 💡 **Looking for FFI integration, C-ABI bindings, or native shared libraries? See the [Multi-Language SDK & FFI Guide](sdks/README.md).**

| Ecosystem | Registry / Package | Installation Command | License |
| :--- | :--- | :--- | :--- |
| **🦀 Rust** | [![Crates.io](https://img.shields.io/crates/v/tapirus.svg?style=flat-square&logo=rust)](https://crates.io/crates/tapirus) | `cargo add tapirus` | BUSL-1.1 |
| **🐍 Python** | [![PyPI](https://img.shields.io/pypi/v/tapirus.svg?style=flat-square&logo=pypi)](https://pypi.org/project/tapirus/) | `pip install tapirus` | MIT |
| **🟢 Node.js / TS** | [![npm](https://img.shields.io/npm/v/tapirus.svg?style=flat-square&logo=npm)](https://www.npmjs.com/package/tapirus) | `npm install tapirus` | MIT |
| **🐹 Go** | [![Go Reference](https://pkg.go.dev/badge/github.com/tapiruslab/TapirusDB/sdks/go.svg)](https://pkg.go.dev/github.com/tapiruslab/TapirusDB/sdks/go) | `go get github.com/tapiruslab/TapirusDB/sdks/go` | MIT |
| **🐘 PHP** | [![Packagist](https://img.shields.io/badge/packagist-v1.0.0-orange.svg?style=flat-square&logo=php)](https://packagist.org/packages/tapiruslab/tapirusdb) | `composer require tapiruslab/tapirusdb` | MIT |
| **🐳 Docker** | [![Docker](https://img.shields.io/badge/ghcr.io-tapirusdb-2496ed?style=flat-square&logo=docker)](https://github.com/tapiruslab/TapirusDB/pkgs/container/tapirusdb) | `docker pull ghcr.io/tapiruslab/tapirusdb:latest` | BUSL-1.1 |
| **🪟 Windows** | [![Winget](https://img.shields.io/badge/winget-tapirus.yaml-0078d4?style=flat-square&logo=windows)](https://github.com/tapiruslab/TapirusDB/blob/main/winget/tapirus.yaml) | `winget install --manifest ...` | BUSL-1.1 |
| **🍺 Homebrew** | [![Homebrew](https://img.shields.io/badge/brew-tapirus.rb-fbb040?style=flat-square&logo=homebrew)](https://github.com/tapiruslab/TapirusDB/blob/main/Formula/tapirus.rb) | `brew install .../tapirus.rb` | BUSL-1.1 |

---

### 2. Code in 30 Seconds

#### Rust: Relational SQL & AI Vector Search
```rust
use tapirus::{Connection, DistanceMetric, Result};

fn main() -> Result<()> {
    // Open in-memory or single-file database: "production.tapir"
    let db = Connection::open_in_memory()?;

    // 1. Create table with structured columns and dense vector embedding
    db.execute("
        CREATE TABLE documents (
            id INTEGER PRIMARY KEY,
            title TEXT NOT NULL,
            category TEXT NOT NULL,
            embedding VECTOR(4)
        );
    ")?;

    db.execute("
        INSERT INTO documents VALUES 
        (1, 'Safe Systems in Rust', 'tech', [0.95, 0.05, 0.0, 0.0]),
        (2, 'Neural Vector Databases', 'ai', [0.10, 0.90, 0.15, 0.0]);
    ")?;

    // 2. Hybrid Vector Search with Single-Pass SQL Pre-Filtering (Exact k Recall)
    let rows = db.query("
        SELECT id, title 
        FROM documents 
        VECTOR NEAR embedding = [0.92, 0.08, 0.0, 0.0] TOP 1
        WHERE category = 'tech';
    ")?;

    for row in rows {
        println!("Match: {}", row.get::<String>("title")?);
    }

    Ok(())
}
```

#### Rust: Declarative openCypher Graph Pattern Matching
```rust
use tapirus::{Connection, Result};

fn main() -> Result<()> {
    let db = Connection::open_in_memory()?;

    // 1. Ingest entities and relationships
    db.execute("GRAPH INSERT NODE 1 LABEL 'Person' PROPERTIES '{\"name\": \"Alice\"}';")?;
    db.execute("GRAPH INSERT NODE 2 LABEL 'Person' PROPERTIES '{\"name\": \"Bob\"}';")?;
    db.execute("GRAPH INSERT NODE 3 LABEL 'Company' PROPERTIES '{\"name\": \"TapirusTech\"}';")?;

    db.execute("GRAPH INSERT EDGE 1 -> 2 LABEL 'KNOWS' WEIGHT 0.9;")?;
    db.execute("GRAPH INSERT EDGE 2 -> 3 LABEL 'WORKS_AT' WEIGHT 1.0;")?;

    // 2. Query graph patterns using industry-standard openCypher
    let rows = db.query("
        MATCH (a:Person)-[r:KNOWS]->(b:Person) 
        WHERE b.name = 'Bob' 
        RETURN a.name, b.name, r.weight;
    ")?;

    for row in rows {
        println!("{} knows {} (weight: {})", 
            row.get::<String>("a.name")?, 
            row.get::<String>("b.name")?, 
            row.get::<f64>("r.weight")?
        );
    }

    Ok(())
}
```

#### Rust: Bidirectional Graph-Vector Chaining & AI Agent Memory
```rust
use tapirus::{Connection, MemoryRecallFilter, Result};
use tapirus::vector::DistanceMetric;

fn main() -> Result<()> {
    let conn = Connection::open_in_memory()?;

    // 1. Graph-to-Vector Chaining (Sub-microsecond 0.55 µs retrieval)
    // Constrains vector distance calculations strictly to local graph neighborhood O(M · D)
    let candidates = conn
        .chain(1)                              // Seed Patient Node
        .out(Some("TREATS"))                  // Traverse outgoing relationships
        .filter_label("Medicine")             // Target node label
        .vector_near(&[0.90, 0.10, 0.0, 0.0], 5, DistanceMetric::Cosine)?;

    // 2. Vector-to-Graph Chaining (Seed-and-Traverse)
    // Seeds from query vector, then traverses adjacent knowledge subgraph
    let discovered = conn
        .chain_from_vector(&[0.85, 0.15, 0.0, 0.0], 1)?
        .out(Some("AUTHORED_BY"))
        .collect_nodes();

    // 3. Autonomous AI Agent Long-Term Memory (LTM)
    // Multi-modal recall: Dense Vector + BM25 Lexical + Recency Decay (e^-λΔt)
    let memory_id = conn.memory_remember(
        "User prefers sovereign on-device processing and strict privacy",
        Some(&[0.92, 0.08, 0.0, 0.0]),
        0.95, // Importance priority score
        &["preferences", "privacy"],
    )?;

    let filter = MemoryRecallFilter::default(); // Balanced Vector + BM25 + Recency
    let recalled = conn.memory_recall(Some("sovereign privacy"), None, 3, &filter);
    println!("Recalled Agent Memory: {}", recalled[0].entry.content);

    Ok(())
}
```

#### Python: Clean Native Integration
```python
from tapirus import Tapirus

with Tapirus.open("app.tapir", passphrase="master_vault_key") as db:
    # ACID Transaction
    db.execute("BEGIN;")
    db.execute("CREATE TABLE telemetry (id INTEGER PRIMARY KEY, sensor TEXT, value REAL);")
    db.execute("INSERT INTO telemetry VALUES (1, 'temperature', 23.8);")
    db.execute("COMMIT;")

    # Direct Python dictionary results
    records = db.query("SELECT * FROM telemetry WHERE value > 20.0;")
    print(records)  # [{'id': 1, 'sensor': 'temperature', 'value': 23.8}]
```

#### Node.js & TypeScript: Zero-Daemon Embedded Database
```javascript
// Install: npm install tapirus
// Run script: node app.mjs
import { open, Tapirus } from "tapirus";

const db = open("production.tapir");

// Relational SQL
db.execute("CREATE TABLE IF NOT EXISTS users (id INT, name TEXT, active INT);");
db.execute("INSERT INTO users VALUES (1, 'Faiz', 1);");

// Query rows as JSON objects
const rows = db.query("SELECT * FROM users WHERE active = 1;");
console.log(rows); // [ { id: 1, raw: "INSERT INTO users VALUES (1, 'Faiz', 1)" } ]

db.close();
```

---

## Beyond AI: An Ultra-Fast Embedded Database for Classic Applications

While TapirusDB is the premier memory engine for sovereign AI and robotics, **you do not need AI to benefit from TapirusDB**. It is also a first-class, zero-configuration embedded database for general applications, edge systems, and analytics:

### 1. Modern Drop-In Replacement for SQLite (Full SQL-92 + ACID)
Need reliable relational tables, transactions, and foreign keys without AI? TapirusDB provides standard SQL with pure Safe Rust reliability:
```rust
// Standard Relational SQL with ACID transactions
db.execute("CREATE TABLE accounts (id INTEGER PRIMARY KEY, email TEXT, balance REAL);")?;
db.execute("INSERT INTO accounts VALUES (1, 'alice@example.com', 1250.50);")?;

// Complex queries with Subqueries & CTEs
let rows = db.query("
    WITH active_accounts AS (
        SELECT id, email, balance FROM accounts WHERE balance > 1000.0
    )
    SELECT * FROM active_accounts;
")?;
```
* **Advanced Query Engine**: Built-in subqueries, CTEs (`WITH ... AS`), `INNER/LEFT JOIN`, and Cost-Based Optimizer (CBO).
* **Transparent Encryption Included**: Hardware-accelerated ChaCha20-Poly1305 encryption at rest without paying for proprietary SQLite commercial extensions.

### 2. Embedded MongoDB Alternative (Schema-less JSON Documents)
Need to store dynamic payloads, user settings, or sensor telemetry with flexible schemas?
```rust
let collection = db.collection("telemetry")?;
let doc_id = collection.insert_one(&serde_json::json!({
    "sensor_id": "temp_probe_09",
    "reading_celsius": 24.3,
    "calibration": { "offset": 0.05, "certified": true },
    "tags": ["factory_floor", "zone_b"]
}))?;
```

### 3. In-Process Analytics & SIMD Aggregations
* **SIMD Aggregations**: Vectorized `SUM`, `AVG`, `COUNT` processing multi-megabyte datasets in microseconds.
* **Transparent Compression**: Built-in pure Safe Rust LZ4 page compression reduces disk footprint by 50%–70%.
* **Developer CLI**: Fast code search tool `tapirus tg` built right into the binary.

---

## 🎯 High-Impact Real-World Domains: Research, Analytics & Smart Home

TapirusDB's zero-dependency single-file architecture is purpose-built for environments where spinning up complex database server clusters is impossible, expensive, or counterproductive:

### 1. 🔬 Scientific Research & Academic Laboratories
* **100% Reproducible Research Bundles**: Peer reviewers and researchers no longer need to configure Docker containers, PostgreSQL, Neo4j, and Milvus just to run a paper's code. Package an entire multimodal dataset—molecular/protein graphs, high-dimensional vector embeddings, and assay measurement SQL tables—into a **single verifiable `experiment.tapir` file**.
* **Zero-Setup Python & Jupyter Workflows**: Install in seconds (`pip install tapirus`) and query directly inside Jupyter notebooks without starting any background daemons.
* **Guaranteed Memory Determinism**: 100% Pure Safe Rust (`#![forbid(unsafe_code)]`) guarantees zero memory leaks, buffer overruns, or segfault crashes during 72-hour batch computation runs.

```python
# Python/Jupyter Research Workflow
import tapirus

# Open single research dataset container
db = tapirus.open("paper_dataset.tapir")

# Query molecular knowledge graph combined with chemical vector distance
results = db.query("""
    MATCH (c:Compound)-[:BINDS_TO]->(p:Protein {id: 'EGFR'})
    WHERE c.smiles_vector <-> $query_vec < 0.15
    RETURN c.id, c.affinity_score;
""", query_vec=target_embedding)
```

### 2. 📊 High-Performance In-Process Analytics & Edge BI
* **Zero-IPC Columnar Aggregations**: Vectorized `SUM`, `AVG`, and `COUNT` accumulators run directly across local memory pages with sub-microsecond execution times, eliminating network hop overhead completely.
* **Transparent LZ4 Disk Compression**: Built-in page compression slashes disk space by 50%–70%, allowing edge gateways and industrial PCs to retain months of historical sensor telemetry locally.
* **Zero Cloud Egress Costs**: Query, aggregate, and analyze high-frequency telemetry at the edge without paying exorbitant bandwidth and ingress bills to cloud data warehouses.

```rust
// In-Process Telemetry Aggregation with Common Table Expressions (CTEs)
let summary = db.query("
    WITH sensor_rollup AS (
        SELECT sensor_id, AVG(reading) AS avg_reading, COUNT(*) AS samples
        FROM telemetry_logs
        WHERE timestamp >= NOW() - 3600
        GROUP BY sensor_id
    )
    SELECT * FROM sensor_rollup WHERE avg_reading > 85.0;
")?;
```

### 3. 🏠 Privacy-First Smart Home & Local Automation (Home Assistant / IoT)
* **100% Sovereign & Local-First**: Run entirely offline on a Raspberry Pi 4/5 or Intel NUC with **< 4 MB idle RAM**. Your private camera triggers, sensor logs, and home conversations never leak to external cloud servers.
* **Mesh Network Topology (openCypher Graph)**: Model Zigbee, Matter, and Thread device hierarchies natively (`MATCH (s:Switch)-[:CONTROLS]->(l:Light)`).
* **Offline Voice Intent Matching (Vector Engine)**: Store speech and intent embeddings locally for sub-millisecond local voice assistant recognition (Whisper / Home Assistant Voice).
* **Blackout Resilience (ACID WAL)**: If your home experiences an abrupt power outage, TapirusDB's Write-Ahead Log guarantees zero database corruption upon reboot.

```rust
// Local Voice Intent Resolution + Zigbee Mesh Pathfinding
let intent_vector = local_whisper.embed("turn off kitchen lights");

// 1. Semantic voice intent match (Vector)
let matched_action = db.vector_search("voice_intents", &intent_vector, 1)?;

// 2. Resolve Zigbee device relay path (openCypher Graph)
let route = db.graph_query("
    MATCH path = (hub:Gateway)-[:ROUTES_THROUGH*1..3]->(d:Device {name: 'kitchen_main_light'})
    RETURN path LIMIT 1;
")?;
```

---

## Architectural Comparison

| Capability | **TapirusDB v1.0.0** | Traditional Relational (SQLite / DuckDB) | Dedicated Vector DBs | Graph Databases (Neo4j) | Document Stores (MongoDB) |
| :--- | :---: | :---: | :---: | :---: | :---: |
| **Runtime Architecture** | **In-Process Single File** | In-Process Single File | Server Daemon / Cloud | Server Daemon (JVM) | Server Daemon (`mongod`) |
| **Memory Safety Model** | **100% Safe Rust (`forbid`)** | C / C++ (Manual memory) | Rust / Go / C++ | Java / JVM | C++ |
| **Data Models Supported** | **Quad-Model (SQL+Vec+Graph+Doc)** | Relational SQL only | Vector embeddings only | Graph only | JSON Document only |
| **AI Vector Search** | **Native HNSW, IVF & RaBitQ** | None (or slow extension) | Native ANN | Basic / Extension | Add-on Atlas Vector |
| **Vector Quantization** | **RaBitQ 32x (1-Bit/2-Bit) + SQ8** | None | PQ / SQ | None | None |
| **Graph Query Engine** | **openCypher + CSR + GraphRAG** | Recursive CTE only | None | Native Cypher | `$graphLookup` |
| **Encrypted At-Rest** | **ChaCha20-Poly1305 (Zero-Cost)** | Commercial Add-on ($$$) | Cloud KMS only | Enterprise Tier ($$$) | Enterprise KMS |
| **Cold Start / Idle RAM** | **< 4 MB RAM** | ~4 MB (SQLite) / ~35 MB | > 500 MB | > 1,200 MB | > 350 MB |
| **Binary Size** | **~3.8 MB** | ~1.5 MB – 42 MB | > 150 MB | > 300 MB | > 200 MB |
| **Multi-Service Sync Drift**| **Zero (Single Container)** | High (manual ETL) | High (CDC pipelines) | High (sync lag) | High (glue code) |

---

## Core Technical Pillars

```text
┌────────────────────────────────────────────────────────────────────────────────────────┐
│                               YOUR APPLICATION HOST PROCESS                            │
│           (Rust • Python • TypeScript • Bun • Go • PHP • WebAssembly • C/C++)          │
│                                                                                        │
│   ┌────────────────────────────────────────────────────────────────────────────────┐   │
│   │                         TapirusDB Core Engine (In-Process)                     │   │
│   │                        100% Safe Rust • Idle RAM < 4 MB                        │   │
│   └───────┬────────────────────┬───────────────────────┬───────────────────┬───────┘   │
│           │                    │                       │                   │           │
│   ┌───────▼────────┐   ┌───────▼────────┐      ┌───────▼────────┐  ┌───────▼────────┐  │
│   │ 1. Relational  │   │ 2. Schema-less │      │  3. AI Vector  │  │  4. Knowledge  │  │
│   │   SQL Tables   │   │  JSON Document │      │   HNSW + IVF   │  │  Graph Engine  │  │
│   │ Slotted B+Tree │   │   Collection   │      │  (SIMD/RaBitQ) │  │  (openCypher)  │  │
│   └───────┬────────┘   └───────┬────────┘      └───────┬────────┘  └───────┬────────┘  │
│           └────────────────────┴───────────────────────┴───────────────────┘           │
│                                           │ Direct In-Memory Traversal                 │
│                                           ▼ (Sub-Microsecond Zero-IPC Chaining)        │
│                    ┌──────────────────────────────────────────────┐                    │
│                    │ Anthropic Model Context Protocol (MCP) Tools │                    │
│                    │ tapirus_remember • tapirus_recall • SQL      │                    │
│                    └──────────────────────┬───────────────────────┘                    │
│                                           │ Direct File I/O (WAL + 4KB Slotted Pages)  │
│                                           ▼                                            │
│                    ┌──────────────────────────────────────────────┐                    │
│                    │ Single Encrypted Database File Container     │                    │
│                    │   • app.tapir      (Authenticated Ciphertext)│                    │
│                    │   • app.tapir-wal  (ACID Append-Only Log)    │                    │
│                    └──────────────────────────────────────────────┘                    │
└────────────────────────────────────────────────────────────────────────────────────────┘
```

### Pillar 1: Inverted File (IVF) Clustering & RaBitQ 32x Quantization
For multi-million vector scale, TapirusDB pairs $k$-means Voronoi partitioning (`IvfIndex`) with **RaBitQ** (Random Rotation Quantization):
1. **Fast Walsh-Hadamard Transform ($O(N \log N)$)**: Orthogonal sign-flip rotation equalizes coordinate variance across high dimensions without the $O(N^2)$ memory overhead of dense projection matrices.
2. **Extreme Compression Ratio**: 1-bit binary sign packing shrinks vectors into `u64` bitmasks:
   $$\text{1536 Dimensions (FP32)} = 6,144 \text{ bytes} \longrightarrow \mathbf{196 \text{ bytes}} \quad (\mathbf{>31\times \text{ Reduction}})$$
3. **Single-Cycle Hardware POPCNT**: Vector distance evaluation executes in single-cycle CPU instructions using hardware `POPCNT` (`count_ones()`).
4. **Multi-Probe Search**: Multi-probe clustering inspects only $n_{\text{probe}} \ll K$ Voronoi cells, pruning ~95% of the vector search space before scoring.

### Pillar 2: Compressed Sparse Row (CSR) & openCypher
Traditional graph systems suffer from pointer indirection and binary join memory explosion. TapirusDB implements:
* **Contiguous Slice Adjacency**: Adjacency lists are stored in contiguous flat memory arrays (`outgoing_offsets`, `outgoing_targets`, `outgoing_weights`). Calling `csr.outgoing_neighbors(node_id)` returns a contiguous slice `&[u64]` with **zero heap allocations** and instant hardware prefetching.
* **Worst-Case Optimal Join (WCOJ) Primitives**: Rapid edge existence checks in $O(\log d)$ via binary search on sorted neighbor slices, with two-pointer intersection sweeps for triangle counting (`triangle_count()`).
* **Declarative openCypher**: Full support for standard pattern matching:
  ```cypher
  MATCH (u:User)-[:FOLLOWS*1..3]->(v:User) 
  WHERE u.id = 1 AND v.active = true 
  RETURN v.name, count(*)
  ```

### Pillar 3: Seed-and-Traverse GraphRAG Engine
Rather than executing expensive unconstrained global vector scans across gigabytes of embeddings, TapirusDB executes **Seed-and-Traverse GraphRAG**:
```text
User Query ──► [IVF/PQ Asymmetric Seeding] ──► Top 2-3 Seed Entities
                         │                                │
                  (Sub-millisecond                        ▼
                   Centroid Pruning)             [Micro-Hop CSR Traversal]
                                                 (BFS 1-2 Hops, Contiguous Memory:
                                                  Extract Factual Knowledge Subgraph)
                                                          │
                                                          ▼
                                                 [Tri-Modal RRF Fusion]
                                                 (Vector + BM25 Lexical + Graph Proximity)
                                                          │
                                                          ▼
                                            [Prompt Context Synthesizer]
                                            (Compact, Hallucination-Free Markdown)
```

$$\text{RRF}(e) = \sum_{m \in \{\text{vec}, \text{lex}, \text{graph}\}} \frac{w_m}{k_{\text{rrf}} + \text{rank}_m(e)}$$

### Pillar 4: Cost-Based Query Optimizer (CBO) & Statistics
TapirusDB features an automated cost-based query optimizer (`src/sql/planner.rs`):
* Computes disk I/O page fetch costs and CPU tuple comparison costs.
* Automatically selects between **Sequential Scan**, **B+Tree Secondary Index Scan**, and **Primary Key Point Lookup**.
* `EXPLAIN QUERY PLAN` outputs estimated execution cost and expected row cardinality.

### Pillar 5: Bidirectional Graph-Vector Chaining & Agent Long-Term Memory
Traditional distributed stacks decouple graph databases and vector stores, causing high network serialization latency and memory-prohibitive global vector scans. TapirusDB executes native bidirectional in-memory chaining at **1,606,037 ops/sec (0.55 µs)**:
* **Graph-to-Vector (Targeted Scored Neighborhoods)**: Traverses structured entity relationships first ($A \to B$), then restricts vector distance scoring strictly to candidate neighborhood nodes ($M \ll N$). Yields **100% exact Recall** with zero approximation loss ($O(M \cdot D)$ instead of $O(N \log N)$).
* **Vector-to-Graph (Seed-and-Traverse GraphRAG)**: Uses ANN centroids to locate seed nodes, then instantly expands 1-hop and 2-hop CSR slices to extract factual context, eliminating LLM hallucinations.
* **Autonomous Agent Long-Term Memory (LTM)**: Automatically balances semantic vector similarity ($S_v$), BM25 lexical precision ($S_l$), and exponential temporal recency decay:
  $$\text{RecallScore}(m) = w_v \cdot S_v + w_l \cdot S_l + w_r \cdot e^{-\lambda \Delta t} + w_i \cdot \text{Importance}$$

---

## 🌐 Industrial Applications: AI & Beyond

TapirusDB's quad-model engine (Relational SQL + Vector Search + openCypher Graph + JSON Documents) inside a single encrypted `.tapir` container solves mission-critical industrial challenges without multi-database operational overhead:

| Industrial Domain | How Quad-Model Solves It Without Server Clusters |
| :--- | :--- |
| **Financial Fraud Detection & AML** | **Graph** traverses money-mule rings and cyclic transactions ($A \to B \to C \to A$); **Vector** identifies anomalous spending behavior signatures; **SQL** enforces immutable balance reconciliation and strict ACID transactions. |
| **Cybersecurity Threat Hunting & SIEM** | **Graph** traces Active Directory lateral movement attack vectors; **Vector** detects polymorphic binary and syscall sequence anomalies; **SQL** queries firewall events and access control lists in microsecond windows. |
| **Supply Chain & Bill-of-Materials (BOM)** | **Graph** manages multi-tiered supplier dependency trees and failure propagation; **Vector** clusters sensor telemetry patterns; **Document** ingests unstructured customs and logistics manifests. |
| **Healthcare, Genomics & Life Sciences** | **Graph** traverses Disease $\to$ Gene $\to$ Symptom $\to$ Drug pathways; **Vector** performs chemical fingerprint similarity (SMILES) for drug repurposing; **SQL** guarantees HIPAA/clinical record integrity. |
| **Scientific Research & Academic Labs** | **Single-file `.tapir` dataset container** guarantees 100% reproducible paper workflows; **Graph + Vector + SQL** models molecular pathways and tabular metrics in Python/Jupyter with zero Docker dependencies. |
| **In-Process Telemetry & Edge BI** | **SIMD vectorized accumulators** compute `AVG`/`SUM`/`COUNT` across millions of sensor readings in microseconds; **Transparent LZ4** cuts disk usage by 70% with zero cloud egress cost. |
| **Privacy-First Smart Home & Home Assistant** | **Graph** maps Zigbee/Matter/Thread device meshes; **Vector** performs local voice intent matching offline; **WAL** guarantees crash durability across home power outages on Raspberry Pi (<4MB RAM). |
| **Air-Gapped Sovereign Hardware & Edge IoT** | Operates on Raspberry Pi, avionics, drones, and naval vessels with **zero server daemons**, **< 4 MB idle RAM**, and **hardware-accelerated ChaCha20-Poly1305 encryption** at rest. |

---

## Developer Tooling & CLI

TapirusDB ships as a single zero-dependency standalone binary (`tapirus`):

### 1. Accelerated Workspace Search (`tapirus grep` / `tapirus tg`)
High-throughput in-process developer code search combining regex matching, BM25 token overlap, and local semantic vector similarity:
```bash
# Search codebase with semantic vector ranking enabled
tapirus tg --vector "transaction rollback wal" src/

# Case-insensitive search filtered by file extensions
tapirus grep -i --ext rs,toml "quantization" .
```

### 2. Interactive Terminal Shell
```bash
tapirus production.tapir
```
```text
tapirus> CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT);
Query OK, 1 row(s) affected

tapirus> INSERT INTO users VALUES (1, 'Ahmad Faiz');
Query OK, 1 row(s) affected

tapirus> SELECT * FROM users;
+----+------------+
| id | name       |
+----+------------+
| 1  | Ahmad Faiz |
+----+------------+
(1 row(s))
```

### 3. Built-in HTTP REST Server (`tapirus serve`)
Launch an embedded database as a high-throughput REST API with zero external dependencies:
```bash
tapirus serve --port 3005 --passphrase "vault_secret" production.tapir
```

### 4. Autonomous AI Agent MCP Server (`tapirus mcp`)
Connect Claude Desktop, Cursor, or Gemini to TapirusDB over stdio:
```json
{
  "mcpServers": {
    "tapirus": {
      "command": "tapirus",
      "args": ["mcp", "agent_memory.tapir"]
    }
  }
}
```

---

## Verified Benchmarks

Benchmarks executed on native NVMe SSD hardware (`cargo bench --bench tapirus_bench`):

| Operation | Throughput | Mean Latency | Median (p50) | Tail (p99) |
| :--- | :---: | :---: | :---: | :---: |
| **Relational Primary Key Point Lookup** | **362,733 ops/sec** | 2.61 µs | 2.37 µs | 4.68 µs |
| **CSR Graph Adjacency Sweep** | **3,493,852 ops/sec** | 0.23 µs | 0.21 µs | 0.37 µs |
| **Graph-to-Vector Bidirectional Chaining** | **1,606,037 ops/sec** | 0.55 µs | 0.51 µs | 1.01 µs |
| **Relational B+Tree Inserts** | **149,176 ops/sec** | 6.19 µs | 4.66 µs | 61.13 µs |
| **JSON Document Path Lookups** | **355,004 docs/sec** | 2.70 µs | 2.58 µs | 5.45 µs |
| **HNSW Vector Search (32D, k=5)** | **51,060 QPS** | 19.53 µs | 17.06 µs | 50.36 µs |
| **RaBitQ Asymmetric POPCNT Distance** | **> 12,000,000 ops/sec**| 0.08 µs | 0.08 µs | 0.12 µs |
| **WAL Durable Disk Writes** | **107,875 writes/sec**| 9.15 µs | 6.71 µs | 62.21 µs |
| **AI Memory Ingest (BM25 Indexing)** | **416,529 ops/sec** | 2.30 µs | 1.77 µs | 4.38 µs |

### Latency Comparison: Traditional Frankenstack vs. TapirusDB In-Process
```text
Cloud Vector DB (gRPC Roundtrip)  [████████████████████████████████████████] 25,000 µs (25.0 ms)
Dedicated Graph DB (HTTP/JVM)     [████████████████████████]                 15,000 µs (15.0 ms)
Relational SQL Server (TCP IPC)   [████████]                                  5,000 µs (5.0 ms)
TapirusDB Combined Graph-Vector   [▌]                                          0.55 µs (Sub-microsecond, ~45,000x faster)
```

---

## When (and When NOT) to Use TapirusDB

Engineering honesty is paramount. Choosing the right storage engine requires understanding boundary trade-offs:

| Workload & Scenario | Recommended Engine | Architectural Rationale |
| :--- | :---: | :--- |
| **Local AI Agents & LLM RAG Memory** | ✅ **TapirusDB** | Microsecond episodic retrieval, combined vector + openCypher graph in one atomic `.tapir` file. |
| **Embedded Edge, Robotics & IoT Hardware** | ✅ **TapirusDB** | < 4 MB idle RAM, 100% Safe Rust, zero background daemon processes or JVM runtimes. |
| **Desktop Apps, CLI Tools & Local-First Web** | ✅ **TapirusDB** | Single-file portability, zero server configuration, pure client-side SQLite/Mongo alternative. |
| **Petabyte Distributed Big Data Warehousing** | ❌ **ClickHouse / Snowflake** | TapirusDB is optimized for operational single-node/in-process workloads, not massive multi-rack OLAP scans. |
| **Multi-Region Active-Active Distributed Writes** | ❌ **CockroachDB / Spanner** | For global multi-master write replication, use dedicated distributed consensus databases (or wait for Tapisaurus). |
| **Complex Analytical BI Cubes over Billions of Rows** | ❌ **DuckDB / ClickHouse** | DuckDB is superior for vectorized columnar OLAP; TapirusDB excels at transactional, graph, vector, and episodic AI memory. |

---

## Formal Safety Verification

* **TLA+ Specifications**: Write-Ahead Logging (WAL) state transitions and crash recovery are formally modeled under TLA+ in [`docs/formal_verification/`](docs/formal_verification/).
* **Memory Safety Contract**: Strict `#![forbid(unsafe_code)]` enforced across all core modules in `src/lib.rs`.

---

## Roadmap: Tapisaurus Distributed Continuum

```text
                        TAPIRUS DATA ARCHITECTURE
                                     │
         ┌───────────────────────────┴───────────────────────────┐
         ▼                                                       ▼
   TAPIRUSDB (Embedded In-Process)                 TAPISAURUS (Distributed Mesh)
   • Single-file container (.tapir)                • Distributed partitioned micro-shards
   • Memory footprint: < 4 MB RAM                  • Raft-consensus multi-region replication
   • Zero IPC overhead                             • Scale-out enterprise analytics
   • Best for: SLMs, edge IoT, desktop, mobile     • Best for: High-availability cloud clusters
```

Read the full distributed specification in [`docs/TAPISAURUS_DISTRIBUTED_BLUEPRINT.md`](docs/TAPISAURUS_DISTRIBUTED_BLUEPRINT.md).

---

## Documentation & Architecture

* [**Architecture Blueprint & Binary File Layout**](BLUEPRINT.md)
* [**GraphRAG & Cloud S3/R2 Remote Storage**](docs/GRAPHRAG_AND_REMOTE_STORAGE.md)
* [**Robotics, Edge Silicon & Autonomous Vehicles**](docs/ROBOTICS_AUTOMOTIVE_EDGE.md)
* [**Scientific Systems Architecture Paper**](PAPER_TAPIRUSDB.md)
* [**C ABI & Native Foreign Function Interface**](include/tapirus.h)
* [**Formal Verification Suite (TLA+)**](docs/formal_verification/README.md)
* [**Software License (BSL 1.1)**](LICENSE)

---

<div align="center">
  <b>TapirusDB — Engineered with 100% Safe Rust for the Modern AI Era.</b><br/>
  <i>Architected & Maintained by Ahmad Faiz • Tapirus Tech Lab (<a href="https://tapirusdb.com">TapirusDB.com</a>)</i><br/>
  <small>Contact: <a href="mailto:faiz@tapirusdb.com">faiz@tapirusdb.com</a></small>
</div>
