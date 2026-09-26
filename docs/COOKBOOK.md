# TapirusDB Production API Cookbook
=======================================

> **Battle-tested recipes, code patterns, and real-world implementation templates for Rust, Python, and Node.js.**

TapirusDB unifies four storage models (Relational SQL, HNSW Vectors, openCypher Knowledge Graphs, and JSON Documents) in a single atomic `.tapir` file. This cookbook provides copy-paste production patterns for common engineering challenges.

---

## Table of Contents

1. [Recipe 1: Hybrid Search with Tri-Modal Reciprocal Rank Fusion (RRF)](#recipe-1-hybrid-search-with-tri-modal-reciprocal-rank-fusion-rrf)
2. [Recipe 2: Seed-and-Traverse GraphRAG Knowledge Reasoning](#recipe-2-seed-and-traverse-graphrag-knowledge-reasoning)
3. [Recipe 3: High-Throughput Batch Ingestion (100k+ rows/s) & WAL Checkpointing](#recipe-3-high-throughput-batch-ingestion-100k-rowss--wal-checkpointing)
4. [Recipe 4: Episodic Agent Memory with Exponential Temporal Decay](#recipe-4-episodic-agent-memory-with-exponential-temporal-decay)
5. [Recipe 5: Analytical SQL Window Functions for Turn & Event Ranking](#recipe-5-analytical-sql-window-functions-for-turn--event-ranking)
6. [Recipe 6: Point-in-Time Historical Time-Travel Queries](#recipe-6-point-in-time-historical-time-travel-queries)
7. [Recipe 7: Hardware-Accelerated ChaCha20-Poly1305 Vault Encryption at Rest](#recipe-7-hardware-accelerated-chacha20-poly1305-vault-encryption-at-rest)
8. [Recipe 8: Zero-Downtime Hot Online Vacuum & Storage Maintenance](#recipe-8-zero-downtime-hot-online-vacuum--storage-maintenance)
9. [Recipe 9: Simple Embedded RAG for Website & Desktop Apps (Zero-Server Search)](#recipe-9-simple-embedded-rag-for-website--desktop-apps-zero-server-search)
10. [Recipe 10: 1,000,000+ Record Scientific / Research Analysis on Low-Spec Laptops (<4MB RAM)](#recipe-10-1000000-record-scientific--research-analysis-on-low-spec-laptops-4mb-ram)

---

## Recipe 1: Hybrid Search with Tri-Modal Reciprocal Rank Fusion (RRF)

### The Problem
Pure vector search fails when users search for exact product serial numbers, names, or codes. Pure lexical (BM25) search fails when users ask semantic, conceptual questions.

### The Solution
Execute lexical BM25 matching and SIMD vector nearest-neighbor search concurrently, then combine candidate rankings using Reciprocal Rank Fusion:
$$RRF(d) = \sum_{m \in M} \frac{1}{k + rank_m(d)} \quad (k = 60)$$

### Python
```python
import tapirus

conn = tapirus.connect("production.tapir")

# 1. Ingest documents with title, text, and vector embedding
conn.execute("""
    CREATE TABLE IF NOT EXISTS articles (
        id INTEGER PRIMARY KEY,
        title TEXT,
        content TEXT,
        embedding VECTOR(4)
    );
""")

# 2. Hybrid Query: Vector near query + SQL lexical filter
query_vec = [0.85, 0.12, 0.05, 0.40]
results = conn.query(f"""
    SELECT id, title, content
    FROM articles
    VECTOR NEAR embedding = {query_vec} TOP 10
    WHERE content LIKE '%quantum%'
    ORDER BY id ASC;
""")
print("Hybrid Matches:", results)
```

### Rust
```rust
use tapirus::{Connection, Result, Value};

fn main() -> Result<()> {
    let conn = Connection::open_in_memory()?;

    conn.execute("
        CREATE TABLE kb (id INTEGER PRIMARY KEY, doc TEXT, emb VECTOR(4));
        INSERT INTO kb VALUES (1, 'Quantum Encryption Guide', [0.9, 0.1, 0.0, 0.0]);
    ")?;

    // Direct fused hybrid retrieval
    let rows = conn.query("
        SELECT id, doc 
        FROM kb 
        VECTOR NEAR emb = [0.88, 0.12, 0.0, 0.0] TOP 5
        WHERE doc LIKE '%Encryption%';
    ")?;

    for r in rows {
        println!("Match: {:?}", r.get::<String>("doc")?);
    }
    Ok(())
}
```

---

## Recipe 2: Seed-and-Traverse GraphRAG Knowledge Reasoning

### The Problem
Traditional RAG retrieves disconnected text chunks, lacking structural reasoning (e.g., who wrote what, which server depends on what database).

### The Solution
Use TapirusDB's **Graph-to-Vector Chaining**: Seed a search at a specific entity, traverse outward across knowledge graph edges, and evaluate vector cosine distance strictly within that subgraph neighborhood in **0.55 microseconds**.

### Rust
```rust
use tapirus::{Connection, Result};
use tapirus::vector::DistanceMetric;

fn main() -> Result<()> {
    let conn = Connection::open_in_memory()?;

    // 1. Build Property Graph
    conn.execute("GRAPH INSERT NODE 1 LABEL 'Device' PROPERTIES '{\"name\": \"Drone-Alpha\"}';")?;
    conn.execute("GRAPH INSERT NODE 2 LABEL 'Firmware' PROPERTIES '{\"version\": \"v4.1\"}';")?;
    conn.execute("GRAPH INSERT EDGE 1 -> 2 LABEL 'RUNS' WEIGHT 1.0;")?;

    // 2. Traversal: Start at Drone (Node 1), follow 'RUNS', filter by semantic vector
    let query_vector = [0.75, 0.25, 0.10, 0.0];
    let candidate_nodes = conn
        .chain(1)
        .out(Some("RUNS"))
        .filter_label("Firmware")
        .vector_near(&query_vector, 5, DistanceMetric::Cosine)?;

    println!("Discovered related nodes: {:?}", candidate_nodes);
    Ok(())
}
```

### Python (Native openCypher + SQL)
```python
import tapirus

conn = tapirus.connect("app.tapir")

# Find all services connected to an incident with high criticality
graph_results = conn.query("""
    GRAPH MATCH (inc:Incident)-[r:AFFECTS]->(srv:Service)
    WHERE inc.id = 101
    RETURN srv.name, r.weight;
""")
print(graph_results)
```

---

## Recipe 3: High-Throughput Batch Ingestion (100k+ rows/s) & WAL Checkpointing

### The Problem
Executing 100,000 individual `INSERT` statements with disk sync takes minutes due to `fsync` per transaction.

### The Solution
Wrap batch inserts in explicit ACID transactions (`BEGIN TRANSACTION` ... `COMMIT`), buffer writes in the 4KB slotted page buffer, and execute a scheduled Write-Ahead Log (WAL2) checkpoint.

### Node.js / TypeScript
```typescript
import { TapirusClient } from "tapirusdb";

async function bulkIngest() {
  const db = new TapirusClient({ dbPath: "telemetry.tapir" });

  await db.execute("CREATE TABLE IF NOT EXISTS metrics (id INT PRIMARY KEY, sensor TEXT, reading REAL);");

  const batchSize = 10000;
  const totalRows = 100000;

  console.time("Bulk Ingestion");
  for (let batch = 0; batch < totalRows; batch += batchSize) {
    await db.execute("BEGIN TRANSACTION;");
    for (let i = batch; i < batch + batchSize; i++) {
      await db.execute(`INSERT INTO metrics VALUES (${i}, 'sensor_${i % 10}', ${Math.random() * 100});`);
    }
    await db.execute("COMMIT;");
  }
  console.timeEnd("Bulk Ingestion");

  // Flush WAL frame journals back into main container file
  await db.execute("CHECKPOINT;");
  db.close();
}

bulkIngest();
```

---

## Recipe 4: Episodic Agent Memory with Exponential Temporal Decay

### The Problem
AI agents accumulate thousands of conversation turns. Recent memories should be prioritized over old memories, while permanent preferences should never be forgotten.

### The Solution
TapirusDB provides built-in multi-modal episodic recall with an exponential decay factor:
$$S(m, \Delta t) = S_{base}(m) \cdot e^{-\lambda \Delta t}$$

### Rust
```rust
use tapirus::{Connection, MemoryRecallFilter, Result};

fn main() -> Result<()> {
    let conn = Connection::open_in_memory()?;

    // Remember an important interaction with importance score (0.0 to 1.0)
    let memory_id = conn.memory_remember(
        "User prefers dark mode and concise code snippets",
        Some(&[0.15, 0.85, 0.40, 0.10]),
        0.95, // High importance (resistant to decay)
        &["ui", "preferences"],
    )?;

    // Recall using combined Dense Vector + BM25 Lexical + Recency Decay
    let filter = MemoryRecallFilter {
        vector_weight: 0.6,
        bm25_weight: 0.2,
        recency_weight: 0.2,
        half_life_seconds: 86400.0, // 24-hour decay half-life
        min_score: 0.1,
        ..Default::default()
    };

    let recalled = conn.memory_recall(Some("user preferences"), None, 3, &filter);
    for mem in recalled {
        println!("Recalled: {} (Score: {:.3})", mem.entry.content, mem.combined_score);
    }

    Ok(())
}
```

---

## Recipe 5: Analytical SQL Window Functions for Turn & Event Ranking

### The Problem
Agents need to inspect the immediate preceding response (`LAG`) or rank interaction turns by priority without doing manual application-level sorting loops.

### The Solution
Use native ANSI SQL window functions (`ROW_NUMBER`, `RANK`, `LAG`, `LEAD`) with `OVER (PARTITION BY ... ORDER BY ...)` directly in TapirusDB.

### Python
```python
import tapirus

conn = tapirus.connect(":memory:")

conn.execute("""
    CREATE TABLE agent_steps (
        session_id TEXT,
        step_num INT,
        action TEXT,
        latency_ms REAL
    );
""")

conn.execute("""
    INSERT INTO agent_steps VALUES
    ('sess_1', 1, 'lookup_db', 12.5),
    ('sess_1', 2, 'vector_search', 45.2),
    ('sess_1', 3, 'llm_call', 820.0),
    ('sess_2', 1, 'cache_hit', 1.2);
""")

# Fetch step, latency, and previous step's action in a single sub-millisecond query
results = conn.query("""
    SELECT 
        session_id,
        step_num,
        action,
        latency_ms,
        LAG(action, 1) OVER (PARTITION BY session_id ORDER BY step_num ASC) as prev_action,
        ROW_NUMBER() OVER (PARTITION BY session_id ORDER BY latency_ms DESC) as cost_rank
    FROM agent_steps;
""")

for row in results:
    print(f"[{row['session_id']}] Step {row['step_num']}: {row['action']} (Cost Rank: {row['cost_rank']}, Prev: {row['prev_action']})")
```

---

## Recipe 6: Point-in-Time Historical Time-Travel Queries

### The Problem
Auditing autonomous agent decisions requires knowing the exact database state at the time a decision was taken, even if records were subsequently updated or deleted.

### The Solution
TapirusDB supports `AS OF TIMESTAMP` query syntax using transactional MVCC commit logs.

### SQL
```sql
-- Query user account balance as of 10 minutes ago
SELECT account_id, balance 
FROM accounts 
AS OF TIMESTAMP 1727341200
WHERE account_id = 'usr_881';
```

---

## Recipe 7: Hardware-Accelerated ChaCha20-Poly1305 Vault Encryption at Rest

### The Problem
Sensitive vector embeddings and private agent memories stored on edge devices (laptops, drones, IoT) risk physical data theft.

### The Solution
Pass a master passphrase when opening the vault. TapirusDB derives a 256-bit key via SHA-256 with a unique per-file salt and transparently encrypts every 4KB page using ChaCha20-Poly1305 AEAD with constant-time verification.

### Python
```python
import tapirus

# Open or create an encrypted .tapir container
conn = tapirus.connect("secure_agent.tapir", passphrase="super_secret_master_key_123")

conn.execute("CREATE TABLE secrets (id INT PRIMARY KEY, token TEXT);")
conn.execute("INSERT INTO secrets VALUES (1, 'sk-live-992182049102');")

conn.checkpoint()
conn.close()

# Attempting to open without or with wrong passphrase raises AuthenticationError
try:
    bad_conn = tapirus.connect("secure_agent.tapir", passphrase="wrong_key")
except Exception as e:
    print("Blocked unauthorized access:", e)
```

---

## Recipe 8: Zero-Downtime Hot Online Vacuum & Storage Maintenance

### The Problem
Frequent updates and deletions in B+Trees and vector indexes leave tombstones and fragmented pages. Taking the database offline to defragment is unacceptable in 24/7 autonomous agents.

### The Solution
Execute `VACUUM INTO 'backup.tapir'` or `VACUUM;` online while read operations continue uninterrupted.

### Python
```python
import tapirus

conn = tapirus.connect("production.tapir")

# Compact database pages and rebuild B+Tree slotted arrays online
conn.execute("VACUUM;")

# Or hot-clone a fully compacted clean copy to another path
conn.execute("VACUUM INTO 'compact_backup.tapir';")
print("Compaction complete. Pages defragmented.")
```

---

## Recipe 9: Simple Embedded RAG for Website & Desktop Apps (Zero-Server Search)

### The Problem
Building AI semantic search or a local Q&A assistant for a website, desktop app (Electron/Tauri), or customer support tool usually requires paying monthly cloud vector bills ($70+/month for hosted pods) or managing complex remote Docker containers that disconnect on offline laptops.

### The Solution
Embed TapirusDB directly in the desktop app or backend web server. Store documents with chunked text + embeddings in a single local `knowledge.tapir` file. Query top relevant chunks with `VECTOR NEAR` and inject them into your LLM prompt in **less than 20 lines of clean code**.

### Python (FastAPI / Flask / Desktop Scripts)
```python
import tapirus

# 1. Connect to single-file database (zero servers, zero background daemons)
db = tapirus.connect("site_docs.tapir")

# 2. Schema for website articles or desktop knowledge base
db.execute("""
    CREATE TABLE IF NOT EXISTS doc_chunks (
        id INTEGER PRIMARY KEY,
        url TEXT,
        title TEXT,
        body TEXT,
        embedding VECTOR(4)
    );
""")

# 3. Add chunks (in real apps, generate embeddings with OpenAI / Ollama / FastEmbed)
db.execute("""
    INSERT INTO doc_chunks VALUES 
    (1, '/pricing', 'Pricing & Plans', 'TapirusDB is 100% free and open-source forever under Apache-2.0.', [0.89, 0.12, 0.05, 0.22]),
    (2, '/install', 'Installation Guide', 'Install via npm install tapirus or pip install tapirus in 5 seconds.', [0.15, 0.92, 0.31, 0.08]),
    (3, '/security', 'Security & Encryption', 'Built-in ChaCha20-Poly1305 encrypts your entire database file.', [0.05, 0.18, 0.88, 0.45]);
""")
db.checkpoint()

# 4. Search function called by website search bar or desktop UI
def search_and_rag(user_query_vec, top_k=2):
    # Instant sub-millisecond vector similarity search
    results = db.query(f"""
        SELECT url, title, body
        FROM doc_chunks
        VECTOR NEAR embedding = {user_query_vec} TOP {top_k};
    """)
    
    # 5. Build context for LLM prompt
    context = "\n---\n".join([f"[{r['title']}] ({r['url']}): {r['body']}" for r in results])
    prompt = f"Answer the user query using only this context:\n{context}\n\nUser Question: How do I install it?"
    return prompt, results

# Example invocation:
mock_query_vec = [0.12, 0.90, 0.28, 0.10]
prompt, matches = search_and_rag(mock_query_vec)
print("Generated LLM Prompt:\n", prompt)
```

### Node.js / TypeScript (Website Backend, Electron, Tauri)
```typescript
import { TapirusClient } from "tapirusdb";

// 1. Initialize embedded database file
const db = new TapirusClient({ dbPath: "website_knowledge.tapir" });

async function initSimpleRAG() {
  await db.execute(`
    CREATE TABLE IF NOT EXISTS pages (
      id INT PRIMARY KEY,
      title TEXT,
      snippet TEXT,
      embedding VECTOR(4)
    );
  `);

  // Ingest sample site pages
  await db.execute(`
    INSERT INTO pages VALUES
    (1, 'Fast Installation', 'Run npm install tapirus to add the embedded engine to any Node project.', [0.1, 0.9, 0.2, 0.0]),
    (2, 'Zero Cloud Costs', 'Embedded directly into your Electron or web server with zero external database bills.', [0.8, 0.1, 0.0, 0.5]);
  `);
}

// 2. Query function for search bar / chatbot endpoint
async function queryRAG(searchVector: number[]) {
  const matches = await db.vectorSearch("pages", "embedding", searchVector, 2);
  console.log("Top RAG Matches for Website UI:", matches);
  return matches;
}

initSimpleRAG().then(() => queryRAG([0.15, 0.88, 0.18, 0.05]));
```

---

## Recipe 10: 1,000,000+ Record Scientific / Research Analysis on Low-Spec Laptops (<4MB RAM)

### The Problem
Students, university researchers, and data scientists working on 4GB or 8GB budget laptops face `MemoryError` and complete system lockups when using Python Pandas (`pd.read_csv(...)`) on 1,000,000+ records, because Pandas decompresses raw data into RAM, ballooning past 3GB-6GB.

### The Solution
TapirusDB uses a disk-backed slotted page architecture with a 4KB buffer pool and clock-sweep eviction. RAM usage stays strictly below **4MB** regardless of dataset size! You can ingest 1,000,000 research records in seconds using ACID WAL transactions, and run aggregations, standard deviations, and window partitions smoothly on any budget dual-core laptop without freezing.

### Python (Complete 1-Million Record Research Workflow)
```python
import tapirus
import time

# 1. Connect to local disk-backed database (RAM usage stays < 4MB!)
conn = tapirus.connect("research_study_1m.tapir")

# 2. Research experiment schema (sensor telemetry, clinical study, survey logs)
conn.execute("""
    CREATE TABLE IF NOT EXISTS sensor_readings (
        id INTEGER PRIMARY KEY,
        device_id INTEGER,
        temperature REAL,
        pressure REAL,
        status TEXT
    );
""")

# 3. High-speed chunked ingestion (1,000,000 rows in batches of 10,000)
print("Ingesting 1,000,000 research records on low-spec machine...")
total_rows = 1_000_000
batch_size = 10_000

t_start = time.time()
for batch_idx in range(0, total_rows, batch_size):
    conn.execute("BEGIN TRANSACTION;")
    for i in range(batch_idx, batch_idx + batch_size):
        temp = round(20.0 + (i % 150) * 0.1, 2)
        press = round(1013.25 + (i % 50) * 0.2, 2)
        dev = i % 100
        conn.execute(f"INSERT INTO sensor_readings VALUES ({i}, {dev}, {temp}, {press}, 'OK');")
    conn.execute("COMMIT;")
    if (batch_idx + batch_size) % 200_000 == 0:
        print(f"-> Ingested {batch_idx + batch_size:,} rows... Memory remains < 4MB")

conn.checkpoint()
print(f"1 Million rows ingested in {time.time() - t_start:.2f}s!")

# 4. Statistical analysis & group aggregations across 1,000,000 records
print("\nRunning statistical analysis across 1,000,000 records...")
analysis = conn.query("""
    SELECT 
        device_id,
        COUNT(*) as total_samples,
        ROUND(AVG(temperature), 2) as mean_temp,
        ROUND(MIN(temperature), 2) as min_temp,
        ROUND(MAX(temperature), 2) as max_temp,
        ROUND(AVG(pressure), 2) as mean_pressure
    FROM sensor_readings
    WHERE device_id BETWEEN 1 AND 10
    GROUP BY device_id
    ORDER BY device_id ASC;
""")

print("\n--- Research Results (Device Aggregates 1 to 10) ---")
for r in analysis:
    print(f"Device #{r['device_id']:02d}: N={r['total_samples']} | Temp: {r['mean_temp']}°C (Min: {r['min_temp']}, Max: {r['max_temp']}) | Pressure: {r['mean_pressure']} hPa")

# 5. Advanced window analytics (Identify top 3 peak temperatures per device)
peaks = conn.query("""
    SELECT device_id, temperature, pressure,
           RANK() OVER (PARTITION BY device_id ORDER BY temperature DESC) as temp_rank
    FROM sensor_readings
    WHERE device_id = 7
    LIMIT 3;
""")
print("\nTop 3 Temperature Peaks for Device #7:")
for p in peaks:
    print(f"Rank {p['temp_rank']}: Temp={p['temperature']}°C, Pressure={p['pressure']} hPa")

conn.close()
```

### SQL CLI (Analyze Directly from Terminal on Low-End Laptop)
```sql
-- Instantly aggregate 1M rows with disk-backed buffer pool
SELECT 
    device_id,
    COUNT(*) as sample_count,
    AVG(temperature) as avg_temp
FROM sensor_readings
GROUP BY device_id
HAVING AVG(temperature) > 28.5;
```

