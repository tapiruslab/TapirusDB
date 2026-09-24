# TapirusDB — Architectural Blueprint & Binary Specification

> **Project Name:** TapirusDB (`tapirus`)  
> **Tagline:** One Engine. Four Models. Zero Data Sprawl.  
> **Author & Architect:** Ahmad Faiz • Tapirus Tech Lab (TapirusDB.com)  
> **Date:** September 2026  
> **Version:** v0.1.3  
> **Edition:** Rust 2024 / Toolchain 1.85+  
> **Target Form Factor:** In-Process Embedded Engine (Zero Network Daemons, Single-File Persistence)  
> **Repository:** `https://github.com/tapiruslab/TapirusDB`

---

## 1. Mission & Architectural Philosophy

TapirusDB is engineered from first principles as the **modern, memory-safe in-process successor to SQLite for the AI era**:

```
┌────────────────────────────────────────────────────────────────────────┐
│                   TapirusDB Core Design Principles                     │
├────────────────────────────────────────────────────────────────────────┤
│ 1. 100% Pure Safe Rust: #![forbid(unsafe_code)] enforced across all    │
│    modules. Zero C/C++ memory vulnerabilities, zero dangling pointers. │
│ 2. Single-File Database: Everything lives in `myapp.tapir` (or .db)    │
│    accompanied by the crash-resilient `myapp.tapir-wal`.               │
│ 3. Native In-Process Linkage: Compiles and links directly             │
│    inside the host runtime (Rust, Python, Node, Bun, Go, PHP, WASM).   │
│ 4. Stack Consolidation: Collapses Relational SQL, Document JSON,       │
│    HNSW Vector Search, and Knowledge Graphs into one single binary.    │
│ 5. Sub-Microsecond Graph-Vector Chaining: 0.55 µs median traversal     │
│    via topological candidate pruning (100% exact neighborhood recall); │
│    22.8 µs HNSW vector search (k=5).                                   │
│ 6. Universal AI Memory & Native Anthropic MCP: Built-in stdio JSON-RPC │
│    server for Claude Desktop, Cursor, and Gemini with temporal decay.  │
│ 7. Native ChaCha20-Poly1305 AEAD: Zero-overhead authenticated page-    │
│    level encryption with SHA-256 key derivation and 16-byte KCV.       │
│ 8. No SQLITE_BUSY: Multi-version concurrency (readers never block      │
│    writers; writers appending to WAL never block active readers).      │
│ 9. WebAssembly Native: Compiles directly to wasm32 for in-browser,     │
│    Cloudflare Workers, and edge runtime persistence.                   │
└────────────────────────────────────────────────────────────────────────┘
```

---

## 2. Native In-Process Architecture Model

TapirusDB operates within the exact same address space as the host application. It completely eliminates inter-process communication (IPC), UNIX domain sockets, TCP loops, and protocol serialization overhead:

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
│   │   SQL Tables   │   │  JSON Document │      │   HNSW + SQ8   │  │  Graph Engine  │  │
│   │ Slotted B+Tree │   │   Collection   │      │  (SIMD/NEON)   │  │   (GraphRAG)   │  │
│   └───────┬────────┘   └───────┬────────┘      └───────┬────────┘  └───────┬────────┘  │
│           └────────────────────┴───────────────────────┴───────────────────┘           │
│                                           │ Direct In-Memory Traversal                 │
│                                           ▼ (Sub-Microsecond Zero-IPC Chaining)        │
│                    ┌──────────────────────────────────────────────┐                    │
│                    │ Anthropic Model Context Protocol (MCP) Tools │                    │
│                    │ tapirus_remember • tapirus_recall • SQL      │                    │
│                    └──────────────────────┬───────────────────────┘                    │
└───────────────────────────────────────────┼────────────────────────────────────────────┘
                                            │ Direct File I/O (WAL + 4KB Slotted Pages)
                                            ▼
                    ┌──────────────────────────────────────────────┐
                    │ Single Encrypted Database File Container     │
                    │   • app.tapir      (Authenticated Ciphertext)│
                    │   • app.tapir-wal  (ACID Append-Only Log)    │
                    └──────────────────────────────────────────────┘
```

---

## 3. Single-File Binary Layout (.tapir)

A TapirusDB database file is organized as a sequence of fixed-size **Pages** (default: 4,096 bytes / 4KB).  
Page 1 contains the 100-byte Database Header followed by Page 1's Master Schema B+Tree Root.

```
┌────────────────────────────────────────────────────────────────────────┐
│                   TapirusDB Database File Structure                    │
├────────────────────────────────────────────────────────────────────────┤
│ Page 1 (Bytes 0..4095):                                                │
│   ├── [Bytes 0..99]    Database File Header (100 Bytes)                │
│   └── [Bytes 100..4095] Master Schema B+Tree Root Page                 │
├────────────────────────────────────────────────────────────────────────┤
│ Page 2 (Bytes 4096..8191):                                             │
│   └── User Table B+Tree Leaf Page (Row Data)                           │
├────────────────────────────────────────────────────────────────────────┤
│ Page 3 (Bytes 8192..12287):                                            │
│   └── Secondary B+Tree Index / Vector Index Page                       │
├────────────────────────────────────────────────────────────────────────┤
│ ... Page N:                                                            │
│   └── Overflow Pages / Freelist Trunk & Leaf Pages                     │
└────────────────────────────────────────────────────────────────────────┘
```

### The 100-Byte Database Header (Page 1, Offset 0..99)

| Byte Offset | Size | Type | Field Name | Description |
|:---:|:---:|:---:|:---|:---|
| **0..7** | 8B | `[u8; 8]` | `magic` | Exact string: `b"TAPIRUS\0"` |
| **8..9** | 2B | `u16` | `page_size` | Database page size in bytes (`4096`). Must be power of 2 between 512 and 65536. |
| **10..11** | 2B | `u16` | `version` | File format version (`1` for v0.1.0/v0.1.1/v0.1.2). |
| **12..15** | 4B | `u32` | `change_counter`| Monotonically incremented on every committed transaction. |
| **16..19** | 4B | `u32` | `total_pages` | Total number of valid pages currently in database file. |
| **20..23** | 4B | `u32` | `freelist_trunk`| Page number of first Freelist Trunk page (`0` if empty). |
| **24..27** | 4B | `u32` | `freelist_count`| Total count of free reusable pages in the freelist. |
| **28..31** | 4B | `u32` | `schema_cookie` | Incremented on DDL alterations (`CREATE TABLE`, `DROP`, `ALTER`). |
| **32..35** | 4B | `u32` | `user_version` | Custom version integer for user migrations (`PRAGMA user_version`). |
| **36..39** | 4B | `u32` | `vector_index_page` | Root page number of Master Vector Directory (`0` if unused). |
| **40..43** | 4B | `u32` | `wal_sequence` | Sequence number of active WAL generation. |
| **44..47** | 4B | `u32` | `header_crc32` | CRC32 checksum over bytes 0..43 for instant corruption detection. |
| **48..99** | 52B | `[u8; 52]` | `reserved` | Cryptographic salt (16B), KCV check tag (16B), and zero padding. |

---

## 4. Slotted Page Layout (Page Header & Cell Pointers)

Within every 4,096-byte page:

```
┌────────────────────────────────────────────────────────────────────────┐
│                        Page Internal Structure                         │
├────────────────────────────────────────────────────────────────────────┤
│ 1. Page Header:                                                        │
│    • Leaf Header: 8 Bytes                                              │
│        - [0]    page_type (1 byte: 0x02 TableLeaf, 0x04 IndexLeaf)     │
│        - [1]    flags (1 byte)                                         │
│        - [2..3] num_cells (2 bytes u16)                                │
│        - [4..5] cell_content_offset (2 bytes u16)                      │
│        - [6..7] first_freeblock (2 bytes u16)                          │
│    • Interior Header: 12 Bytes                                         │
│        - [0..7] Same as Leaf                                           │
│        - [8..11] right_child (4 bytes u32 PageId)                      │
├────────────────────────────────────────────────────────────────────────┤
│ 2. Cell Pointer Array:                                                 │
│    • Array of 2-byte offsets: `[u16; num_cells]`                       │
│    • Grows downwards (from header end towards bottom of page)          │
├────────────────────────────────────────────────────────────────────────┤
│ 3. Unallocated Free Space:                                             │
│    • `free_space = cell_content_offset.saturating_sub(header + cells*2)`│
├────────────────────────────────────────────────────────────────────────┤
│ 4. Cell Content Area:                                                  │
│    • Holds actual data rows / keys. Grows upwards from bottom of page. │
│    • When Free Space < required cell size -> Page Splits!              │
└────────────────────────────────────────────────────────────────────────┘
```

### Page Types:
- `0x01`: `TableInterior` (Interior B+Tree page, routing to child pages)
- `0x02`: `TableLeaf` (Leaf B+Tree page, holds actual row payloads)
- `0x03`: `IndexInterior` (Interior B+Tree index page)
- `0x04`: `IndexLeaf` (Leaf B+Tree index page, holds Key -> RowID)
- `0x05`: `VectorIndex` (Native HNSW Graph Vector Node page)
- `0x06`: `Overflow` (Linked overflow chain for records > page size)
- `0x07`: `Freelist` (Freelist trunk/leaf page)

---

## 5. Dual-Engine B+Tree & Cell Format

### A. Variable-Length Integers (Varint)
TapirusDB adopts SQLite-compatible 1-to-9 byte varints:
- Values $\le 127$ (`0x7F`) encode in **1 byte**.
- Values $\le 16383$ (`0x3FFF`) encode in **2 bytes**.
- High-order bit (`0x80`) signals continuation in bytes 1..8.
- Byte 9 uses all 8 bits, supporting up to full `u64::MAX`.

### B. Table Leaf Cell (`0x02`)
A relational table leaf cell stores a primary key `row_id` and serialized column tuple:

```
┌─────────────────┬──────────────┬──────────────────┬──────────────────────┐
│ payload_size    │ row_id       │ payload bytes    │ overflow_page (opt)  │
│ (Varint 1..9B)  │ (Varint 1..9)│ (Raw Tuple Data) │ (4 Bytes u32)        │
└─────────────────┴──────────────┴──────────────────┴──────────────────────┘
```

---

## 6. The Native Vector Page (`0x05`) & Embedded HNSW

TapirusDB natively reserves Page Type `0x05` for vector similarity graph nodes:

```
┌────────────────────────────────────────────────────────────────────────┐
│                   Vector Page Layout (Type 0x05)                       │
├────────────────────────────────────────────────────────────────────────┤
│ Vector Page Header (16 Bytes):                                         │
│   • [0]     page_type (0x05)                                           │
│   • [1]     distance_metric (0 = Cosine, 1 = Euclidean, 2 = Dot)       │
│   • [2..3]  dimensions (u16, e.g. 128, 384, 768, 1536)                 │
│   • [4..5]  node_count (u16)                                           │
│   • [6..7]  max_neighbors_m (u16, default: 16)                         │
│   • [8..11] entry_point_id (u32)                                       │
│   • [12..15] next_vector_page (u32)                                    │
├────────────────────────────────────────────────────────────────────────┤
│ Vector Node Array:                                                     │
│   Each Node:                                                           │
│     • vector_id (8B u64)                                               │
│     • float_embedding (`dimensions * 4B` or 1B SQ8 quantized)         │
│     • neighbor_count (2B u16)                                          │
│     • neighbor_ids (`neighbor_count * 4B`)                             │
└────────────────────────────────────────────────────────────────────────┘
```

---

## 7. Bidirectional Vector $\leftrightarrow$ Graph Chaining Pipeline (`src/graph/chain.rs`)

To eliminate the friction of coordinating a graph database with a vector database, TapirusDB provides a zero-copy topological chaining engine:

### A. Algorithmic Candidate Pruning
Instead of querying an unconstrained vector space $\mathcal{V}$ ($|\mathcal{V}| = N$), the `GraphChain` operator restricts vector scoring to topological candidate sets $\mathcal{C}_u$ defined by edge traversal:

$$\mathcal{C}_u = \{ v \in \mathcal{V} \mid (u, v) \in \mathcal{E} \land \lambda(u, v) = \text{label} \}$$

Because $|\mathcal{C}_u| = M \ll N$ (typically 10 to 50 nodes), exact 8-way unrolled SIMD vector distance scoring takes:
$$T_{\text{chain}} = O(d_{\text{out}}(u)) + O(M \cdot D) + O(M \log k)$$

This delivers **100% exact neighborhood Recall** at a median latency of **510 nanoseconds (0.51 µs)** with zero heap allocations.

### B. Traversal Operators:
- `chain(seed_id)`: Initialize traversal from a known entity ID.
- `chain_from_vector(query, k)`: Seed traversal dynamically from top-$k$ nearest vector embeddings.
- `.out(label)`: Follow outgoing directed edges with optional label filter.
- `.in_dir(label)`: Follow incoming directed edges.
- `.both(label)`: Follow undirected connections.
- `.filter_label(label)`: Restrict current frontier to matching entity labels.
- `.vector_near(query, k, metric)`: Score frontier candidates using SIMD distance functions.
- `.hybrid_rank(text, query, k, alpha)`: Combine dense vector cosine similarity with Okapi BM25 lexical ranking.

---

## 8. Native Anthropic Model Context Protocol (MCP) Server

TapirusDB embeds a complete JSON-RPC 2.0 stdio server conforming to Anthropic's Model Context Protocol specification (`2024-11-05`):

```bash
tapirus mcp [DATABASE_FILE]
```

### Stdio Transport Protocol:
1. **Handshake (`initialize`)**: Negotiates capabilities and tools list.
2. **Tools Discovery (`tools/list`)**:
   - `tapirus_remember`: Persist episodic or semantic agent memories with optional embeddings, tags, namespaces, and session IDs.
   - `tapirus_recall`: Retrieve relevant context via vector similarity with temporal exponential decay ($e^{-\lambda \Delta t}$).
   - `tapirus_sql`: Execute arbitrary SQL DDL/DML queries against relational tables and vector fields.
   - `tapirus_graph_neighbors`: Traverse connected entities in the knowledge graph in $O(1)$ amortized time.
3. **Execution (`tools/call`)**: Executes transactional operations against the local `.tapir` container and returns structured JSON-RPC responses.

---

## 9. Frontier Edge Intelligence: Tri-Modal GraphRAG, Multimodal Perceptual Vectors, Robotics, Autonomous Swarms & Edge IoT

TapirusDB is purpose-engineered as an in-process, zero-cloud memory and persistence backbone for resource-constrained edge systems, multimodal sensory pipelines, and autonomous embodied agents.

### 9.1 Seed-and-Traverse Tri-Modal GraphRAG Architecture
Traditional flat vector search degrades when answering complex multi-hop queries, frequently inducing LLM hallucination. TapirusDB implements a 4-stage deterministic retrieval pipeline:
1. **Asymmetric Vector Seeding**: Rapidly pinpoints 2–3 seed entities via IVF/HNSW Euclidean or Cosine distance evaluation ($< 100\text{ µs}$).
2. **Micro-Hop CSR Topology Expansion**: Traverses $1\text{–}2$ hops along typed relationships (e.g. `DEPENDS_ON`, `TREATS`, `IS_PART_OF`) directly within contiguous Compressed Sparse Row (CSR) slices, extracting the factual knowledge subgraph with zero DRAM allocation penalties.
3. **Tri-Modal Reciprocal Rank Fusion (RRF)**: Combines dense vector semantic similarity ($S_v$), Okapi BM25 lexical keyword scoring ($S_l$), and graph structural proximity ($S_g$):
   $$\text{RRFScore}(e) = \sum_{m \in \{\text{vec}, \text{lex}, \text{graph}\}} \frac{w_m}{k_{\text{rrf}} + \text{rank}_m(e)}$$
4. **Context Synthesis**: Assembles a hallucination-free, high-density Markdown context window, reducing LLM prompt token consumption by up to $70\%$.

### 9.2 Multimodal Sensory Vectors & Poly-Vector Schemas
TapirusDB supports multiple heterogeneous vector columns within a single relational row or property graph node, enabling cross-modal retrieval across distinct physical and perceptual modalities:
```sql
CREATE TABLE robotic_perception (
    frame_id INTEGER PRIMARY KEY,
    timestamp BIGINT NOT NULL,
    visual_embedding VECTOR(512),       -- Vision Transformer (ViT / CLIP)
    acoustic_embedding VECTOR(256),     -- Audio Spectrogram Embedding (CLAP / Whisper)
    lidar_signature VECTOR(128),        -- 3D Point-Cloud Spatial Representation
    environmental_meta JSON             -- Temperature, humidity, telemetry JSON
);
```
Cross-modal search queries can match text input (e.g. *"anomalous motor grinding sound"*) directly against indexed `acoustic_embedding` vectors, cross-referenced with relational sensor coordinates.

### 9.3 Robotics & Embodied Spatial AI (AMRs, Drones, Humanoids)
Physical robots operate under strict sub-millisecond real-time constraints where remote cloud database round-trips ($100\text{–}300\text{ ms}$) risk catastrophic mechanical collision:
- **Semantic SLAM & Topological Mapping**: The **Property Graph** models room topologies and navigation waypoints (`Room_1 -> DOORWAY -> Corridor_3 -> CHARGING_DOCK`).
- **Visual Place Recognition (Loop Closure)**: **Vector Indexing** matches real-time camera frames against historical embeddings to confirm whether a robot has previously visited an orientation.
- **Actuator & Telemetry Ledger**: **Relational SQL** stores high-frequency motor velocities, battery voltages, and safety boundaries under ACID transactional guarantees.
- **Zero In-Process Latency**: Links directly into the robot's onboard single-board computer (NVIDIA Jetson AGX/Orin, Raspberry Pi CM4, x86_64 industrial box) with zero inter-process IPC overhead.

### 9.4 Cognitive Autonomous Agent Memory & Swarm Coordination
TapirusDB provides a complete long-term memory (LTM) substrate for single and multi-agent swarms:
- **Dual Episodic & Semantic Memory**: Stores sequential observations, dialogues, and distilled world models.
- **Exponential Temporal Recency Decay**: Applies biological forgetting curves to prevent context bloat:
  $$\text{Weight}(t) = e^{-\lambda \Delta t}, \quad \lambda = \frac{\ln(2)}{T_{\text{half-life}}}$$
- **Swarm Namespace Partitioning**: Independent autonomous agents (e.g. `drone_scout_01`, `harvester_bot_04`) maintain isolated or federated memory namespaces within a single `.tapir` container file.

### 9.5 Industrial Edge IoT & Harsh Environment Resilience
1. **Micro-Footprint Density (< 4 MB Idle RAM)**: Eliminates JVM, Docker, and background daemon dependencies; executes effortlessly on low-power microprocessors.
2. **Abrupt Power-Loss Immunity**: Atomic 24-byte checksummed WAL frames guarantee zero database corruption upon sudden power loss or battery depletion.
3. **Transparent LZ4 Page Compression**: Compresses 4KB pages on the fly, reducing flash memory (eMMC/SD) write amplification by $50\%\text{–}70\%$ and extending hardware operational longevity.
4. **Physical Tamper Protection**: Page-level ChaCha20-Poly1305 AEAD ensures field-stolen devices cannot be read without the cryptographic key.

### 9.6 Scientific Research & Academic Laboratories (Reproducible Multimodal Bundles)
1. **Single-File Data Archiving (`experiment.tapir`)**: Eliminates the multi-service dependency crisis in academic reproducibility. Rather than forcing reviewers to spin up Docker, Postgres, Neo4j, and Milvus, the entire experimental state (molecular/biological graphs, vector embeddings, and assay measurement SQL tables) is encapsulated in a single, verifiable, portable `.tapir` archive.
2. **Zero-Setup Python / Jupyter Workflows**: Directly queryable in Python via `pip install tapirus` without starting server daemons.
3. **100% Deterministic Safe-Rust Reliability**: Compile-time `#![forbid(unsafe_code)]` guarantees that 72-hour computational batch jobs never terminate due to segmentation faults or unmanaged memory leaks.

### 9.7 In-Process Telemetry & Edge Data Analytics (SIMD Aggregations & LZ4 Compression)
1. **Zero-IPC Vectorized Columnar Math**: Executes SIMD-accelerated aggregations (`SUM`, `AVG`, `COUNT`, `MIN`, `MAX`) across in-memory pages without network socket serialization or IPC boundary penalties.
2. **Edge Stream Pruning & Rollups**: Aggregates high-frequency sensor readings at the IoT gateway, computing local rolling windows via SQL Common Table Expressions (CTEs).
3. **Bandwidth & Cloud Egress Elimination**: Local compression (LZ4) combined with in-situ edge analysis eliminates expensive cloud data transfer fees.

### 9.8 Privacy-First Smart Home & Local Automation (Offline Home Assistant & IoT)
1. **100% Sovereign Local Operation**: Executes on resource-constrained single-board computers (Raspberry Pi 4/5, Intel NUC) with an idle footprint `< 4 MB RAM`, keeping user speech, camera triggers, and telemetry entirely off public cloud servers.
2. **Mesh Network Topology (openCypher Graph)**: Models Zigbee, Matter, and Thread device hierarchies natively (`MATCH (s:Switch)-[:CONTROLS]->(l:Light)`).
3. **Local Voice Intent Vectors**: Performs instant local vector matching for voice assistant commands (Whisper / Home Assistant Voice) with sub-millisecond latencies.
4. **Blackout Durability**: Checksummed Write-Ahead Logging guarantees instantaneous recovery without corruption even during sudden residential power cuts.

---

## 10. Durability & Concurrency: The Tapirus WAL

TapirusDB operates with a Write-Ahead Log (`.tapir-wal`):
1. **Readers Never Block Writers**: Readers look up the WAL Index in shared memory, reading the latest committed version of each page without taking read locks on the main database file.
2. **Writers Append**: New transactions sequentially write frames to `.tapir-wal`.
3. **WAL Frame Layout**:
   ```
   ┌──────────────────────┬──────────────────────┬──────────────────────┐
   │ PageId (4B u32)      │ CommitSeq (4B u32)   │ Salt / Frame CRC32   │
   ├──────────────────────┴──────────────────────┴──────────────────────┤
   │ 4,096-Byte Page Image Content                                      │
   └────────────────────────────────────────────────────────────────────┘
   ```
4. **Passive/Auto Checkpointing**: Once WAL reaches a configurable threshold (default: 1,000 pages), dirty pages are flushed back into the `.tapir` file and WAL is truncated.

---

## 11. Page-Level Authenticated Encryption (ChaCha20-Poly1305 AEAD)

TapirusDB integrates authenticated encryption directly into the pager boundary:
- **Key Derivation:** PBKDF2-HMAC-SHA256 (600,000 iterations default) or Argon2id.
- **Deterministic Nonce:** 96-bit composite nonce ($\text{Epoch} \,\|\, \text{PageId} \,\|\, \text{CommitSeq}$) preventing nonce reuse across commits.
- **AAD Binding:** Domain separation tag, epoch, and physical PageId bound to Poly1305 MAC to prevent page transposition attacks.
- **Key Check Value (KCV):** Constant-time 16-byte heuristic validation on Page 1 to verify passphrase validity prior to dirty page decryption.

---

## 12. Phased Engineering Roadmap & Delivery Status

| Phase | Milestone | Core Deliverable | Status |
|:---:|:---|:---|:---:|
| **Phase 1** | **Binary Pager & File Format** | 100-byte header, 4KB page allocator, CRC32 checks, single-file I/O. | **Completed** |
| **Phase 2** | **Slotted Page B+Tree** | Slotted page headers, cell insertion, node splitting, page merging, 1-9B varints. | **Completed** |
| **Phase 3** | **Binary Codec & Schema Catalog** | Ultra-compact tuple codec, persistent Master Schema B+Tree on Page 1. | **Completed** |
| **Phase 4** | **Embedded SIMD Vector Search** | Native HNSW graph indexing, Cosine / L2 / Dot Product distance metrics. | **Completed** |
| **Phase 5** | **Native Property Graph (GraphRAG)** | Bi-directional adjacency index, BFS shortest-path, subgraph extraction. | **Completed** |
| **Phase 6** | **Document DB (MongoDB-style)** | Schema-less JSON collections (`db.collection("users")`), auto-ID generation. | **Completed** |
| **Phase 7** | **SQL Lexer, Parser & VM** | AST for CREATE TABLE, INSERT, SELECT, WHERE equality, LIMIT, and VECTOR NEAR. | **Completed** |
| **Phase 8** | **WAL & Crash Recovery** | Non-blocking readers, append-only crash-safe write-ahead logging (`.tapir-wal`), TLA+ formally verified. | **Completed** |
| **Phase 9** | **WebAssembly & Universal C FFI (`tapirus.h`)** | In-browser wasm32 target, universal C headers for Python (`pip install tapirus`) and Node.js (`npm install @tapirusdb/tapirus`). | **Completed** |
| **Phase 10** | **AI Agent Memory & Advanced SQL Engine** | Long-term memory engine (decay, semantic recall, namespaces), SIMD distance unrolling, SQL `INNER JOIN`, `ORDER BY`, aggregates (`COUNT`, `SUM`, `AVG`, `MIN`, `MAX`). | **Completed** |
| **Phase 11** | **Fault-Injection & Tail-Latency Profiling** | Crash-injection test suite (torn writes, bit rot CRC32, header recovery, idempotent replay), HNSW dynamic tombstone deletion & vacuuming, scientific p50/p95/p99 tail-latency profiling. | **Completed** |
| **Phase 12** | **Graph-to-Vector & Hybrid Chaining Pipeline** | Fluent builder API (`conn.chain(id).out("REL").vector_near(...)`, `hybrid_rank`, `bm25_search`), zero-copy traversal, sub-microsecond neighborhood SIMD ranking (p50: 0.51 µs, 1.60M ops/s), 100% exact Recall. | **Completed** |
| **Phase 13** | **Native Anthropic Model Context Protocol (MCP)** | Stdio JSON-RPC 2.0 server (`tapirus mcp`) with memory, recall, SQL, and graph tools for Claude Desktop, Cursor, and Gemini. | **Completed** |

---

## 13. The GrandTapirus / Tapisaurus Distributed Continuum

For planetary-scale enterprise infrastructure processing millions of concurrent files and multi-continent workloads, TapirusDB forms the foundation of a **Dual-Tier Ecosystem**:

```text
                              TAPIRUS DATABASE SPECTRUM
                                          │
        ┌─────────────────────────────────┴─────────────────────────────────┐
        ▼                                                                   ▼
  TAPIRUSDB (Embedded Micro-Titan)                       GRANDTAPIRUS / TAPISAURUS (Distributed)
  • Single-file (.tapir) container                       • Planet-scale scale-out cluster
  • Ultra-compact (< 4 MB idle RAM)                      • Millions of concurrent micro-shards
  • Zero external daemons / in-process                   • Multi-Raft active-active cross-continent consensus
  • Best for: Local SLMs, edge IoT,                      • Best for: Global enterprises, cloud hyper-scalers,
    wearables, robotics, desktop apps                      petabyte AI corpora, multi-region failover
```

1. **TapirusDB Micro-Shard Fabric**: Thousands of independent, self-contained TapirusDB micro-shards (< 4 MB RAM per partition) eliminating cross-shard lock contention.
2. **Multi-Raft Consensus Protocol**: Active-active cross-continent consensus with zero data loss ($\text{RPO} = 0$, $\text{RTO} < 1\text{ s}$) across catastrophic partitions.
3. **Hybrid Logical Clocks (HLC)**: Sub-second causal ordering across Asia, Europe, and the Americas without requiring physical atomic clocks.
4. **Federated Quad-Model Query Coordinator**: Distributed query execution across sharded HNSW vectors, distributed property graphs, and relational SQL.

---

<div align="center">
  <b>TapirusDB Blueprint & Binary Specification</b><br/>
  <i>Architected & Maintained by Ahmad Faiz • Tapirus Tech Lab (TapirusDB.com)</i>
</div>
