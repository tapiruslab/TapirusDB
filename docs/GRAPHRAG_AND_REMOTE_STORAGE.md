# 🧠 TapirusDB Accelerated GraphRAG & Serverless Cloud Storage Guide

> **Architectural Specification & Technical Manual**  
> **Author & Architect:** Ahmad Faiz (TapirusLab / TapirusDB)  
> **Specification Version:** 1.0.0 (v0.1.2)  
> **Scope:** Seed-and-Traverse GraphRAG, Product Quantization (PQ), Tri-Modal RRF, S3/R2 Remote Pager, and Cloudflare Edge Workers

---

## 1. 🌟 The GraphRAG Paradigm Shift

### The Problem with Naive Vector RAG
Standard Retrieval-Augmented Generation (RAG) compares query vectors against thousands of disconnected text chunk embeddings ($O(N \cdot D)$). This naive approach suffers from three major flaws:
1. **High Latency & Compute Costs:** High-dimensional floating-point vector comparisons consume excessive CPU cycles and memory bandwidth.
2. **Semantic Drift & Needle-in-a-Haystack:** The model retrieves isolated chunks that lack factual causality or relationship context.
3. **Bloated Context Windows:** Developers are forced to inject 30–50 chunks to avoid missing context, driving up LLM token costs and latency.

### The TapirusDB Solution: "Seed-and-Traverse"
TapirusDB eliminates these issues by coupling high-speed **Product Quantization (PQ)** vector indexing with an embedded **Property Graph**:

```text
┌─────────────────────────────────────────────────────────────────────────────┐
│                      ALIRAN GRAPHRAG TAPIRUSDB                              │
│                                                                             │
│   User Query ──► [PQ Asymmetric Seeding] ──► Top 2-3 Seed Entities          │
│                            │                          │                     │
│                     (Jadual Carian ADC                ▼                     │
│                      Sub-milisaat)          [Micro-Hop Traversal]           │
│                                             (BFS 1-2 Hops, O(1) Edge Hop:   │
│                                              Kumpul Hubungan Fakta)         │
│                                                       │                     │
│                                                       ▼                     │
│                                             [Tri-Modal RRF Fusion]          │
│                                             (Vektor + BM25 + Graf Proximity)│
│                                                       │                     │
│                                                       ▼                     │
│                                        [Prompt Context Synthesizer]         │
│                                        (Markdown Padat Sedia-Suntik LLM)    │
└─────────────────────────────────────────────────────────────────────────────┘
```

1. **Step 1 — Fast Seed Discovery (PQ):** Instead of scanning every vector, query embeddings are compared using precomputed Asymmetric Distance Computation (ADC) lookup tables in $< 1\text{ ms}$, selecting just the top 2–3 seed entities.
2. **Step 2 — Micro-Hop Adjacency Traversal:** The engine follows explicit relationship edges ($O(1)$ pointer jumps in memory) up to $N$ hops (typically 1–2 hops) to collect all connected factual entities.
3. **Step 3 — Tri-Modal Reciprocal Rank Fusion (RRF):**
   Three discrete rank lists are fused using mathematical RRF:
   $$\text{RRF Score}(e) = \sum_{m \in \{\text{vector}, \text{lexical}, \text{graph}\}} \frac{w_m}{k_{\text{rrf}} + \text{rank}_m(e)}$$
   - $R_{\text{vector}}$: Semantic embedding similarity
   - $R_{\text{lexical}}$: BM25 keyword matching on entity labels and properties
   - $R_{\text{graph}}$: Topological graph proximity and accumulated relationship edge weights
4. **Step 4 — Prompt-Ready Markdown Synthesis:**
   Serializes the retrieved subgraph into structured Markdown containing explicit entity definitions and typed relationships (`(Entity A) -[RELATION]-> (Entity B)`), giving the LLM airtight factual grounding.

---

## 2. 💻 Code Example: GraphRAG in Rust

```rust
use tapirus::{Connection, GraphRagConfig, Result};

fn main() -> Result<()> {
    let db = Connection::open_in_memory()?;

    // 1. Ingest Knowledge Graph entities with dense vectors
    db.graph_add_node_with_vector(
        1,
        "Albert Einstein",
        r#"{"field": "Theoretical Physics", "nobel_year": 1921}"#,
        Some(&[0.92, 0.15, 0.05]),
    )?;

    db.graph_add_node_with_vector(
        2,
        "General Relativity",
        r#"{"field": "Physics", "year": 1915}"#,
        Some(&[0.90, 0.18, 0.02]),
    )?;

    db.graph_add_node_with_vector(
        3,
        "Gravitational Waves",
        r#"{"predicted_by": "General Relativity", "detected": 2015}"#,
        Some(&[0.85, 0.22, 0.08]),
    )?;

    // 2. Establish relationship edges
    db.graph_add_edge(1, 2, "FORMULATED", 1.0, "{}")?;
    db.graph_add_edge(2, 3, "PREDICTS", 0.95, "{}")?;

    // 3. Execute accelerated GraphRAG query
    let config = GraphRagConfig::default()
        .with_seeds(2)
        .with_max_hops(2)
        .with_limit(3)
        .with_weights(0.5, 0.3, 0.2);

    let query_vector = [0.91, 0.16, 0.04];
    let context = db.graph_rag_query("Einstein Relativity", Some(&query_vector), &config)?;

    println!("Synthesized Context for LLM:\n{}", context.prompt_context);

    Ok(())
}
```

---

## 3. 🌐 Cloud-Native S3 / Cloudflare R2 Remote Storage (`RemotePager`)

### The Serverless Dilemma
In serverless runtimes (AWS Lambda, Cloudflare Workers, Google Cloud Run), downloading an entire 500MB database file on cold-start causes unacceptable delays.

### The TapirusDB Remote Pager
TapirusDB abstracts storage I/O into the `RemotePager`. Using **HTTP Range Requests**, it streams only the 4KB pages needed for the current query:

```text
┌───────────────────────────┐                HTTP Range Request                ┌──────────────────────────┐
│       TapirusDB           │ ── Range: bytes=4096-8191 (Page 2) ───────────► │  Cloud Object Storage    │
│  (AWS Lambda / Cloudflare)│ ◄── [4,096 bytes B+Tree Leaf Block] ─────────── │  (AWS S3 / Cloudflare R2)│
│                           │                                                 └──────────────────────────┘
│   ┌───────────────────┐   │
│   │ In-Memory Cache   │ ──┼──► Subsequent queries hit local memory cache in 0.00 ms!
│   └───────────────────┘   │
└───────────────────────────┘
```

### Key Architectural Benefits:
- **Instant Cold Start:** Initial header fetch takes $< 20\text{ ms}$; no full file download.
- **Zero Redundant Bandwidth:** Reads only the exact B+Tree or vector index page nodes accessed by the SQL or Graph query.
- **In-Memory Cache:** All fetched 4KB blocks are cached locally, achieving 0ms latency for subsequent index traversals.

---

## 4. ⚡ Deploying on Cloudflare Workers

TapirusDB compiles to WebAssembly and runs in V8 worker isolates with sub-5ms cold starts.

### Quick Deployment Steps:
1. Navigate to `examples/cloudflare-worker/`
2. Install dependencies:
   ```bash
   npm install
   ```
3. Run locally:
   ```bash
   npx wrangler dev
   ```
4. Query the worker:
   ```bash
   curl -X POST http://localhost:8787/graph-rag \
     -H "Content-Type: application/json" \
     -d '{"query": "Physics Theories", "topSeeds": 2, "maxHops": 2}'
   ```
5. Deploy to global edge:
   ```bash
   npx wrangler deploy
   ```
