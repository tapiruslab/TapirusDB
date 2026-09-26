# Building Autonomous AI Agent Memory in 30 Minutes with TapirusDB

> **Why build agent memory on three different databases when one embedded engine does it all with 0 latency, 0 cloud cost, and <4MB RAM?**

Modern AI Agents require three fundamental memory modalities:
1. **Episodic Memory**: Sequential interaction history, tool executions, and turn-by-turn conversation flow.
2. **Semantic Memory**: High-dimensional vector embeddings for fuzzy concept retrieval and RAG.
3. **Associative / Knowledge Graph Memory**: Entity-to-entity relationships and topological clustering to understand connections between people, places, and facts.

In traditional architectures, developers stitch together **SQLite + Pinecone/LanceDB + Neo4j**. This results in fragile synchronization, high network latency (50-200ms roundtrip), and recurring cloud bills.

**TapirusDB** solves this by unifying all three memory models into a single, zero-dependency embedded `.tapir` vault running directly inside your application thread.

---

## Architecture: The Quad-Model Agent Memory

```
┌─────────────────────────────────────────────────────────────────┐
│                  Autonomous AI Agent Runtime                    │
└────────────────────────────────┬────────────────────────────────┘
                                 │
     Direct In-Process C-ABI / Python / Node.js Sub-Millisecond
                                 │
┌────────────────────────────────▼────────────────────────────────┐
│               TapirusDB Embedded Engine (< 4MB RAM)             │
│                                                                 │
│  1. Relational & Window Analytics  (Turn ranking, LAG/LEAD)     │
│  2. Native SIMD Vector Index       (Cosine, L2 Top-K Search)    │
│  3. GraphBLAS Topological Engine   (Louvain, Centrality, WCC)   │
│  4. Document / JSON Store          (Dynamic metadata payloads)  │
└────────────────────────────────┬────────────────────────────────┘
                                 │
                    Single File: `agent_vault.tapir`
                    (4KB Slotted Pages + WAL2 + ChaCha20)
```

---

## 1. Installation

### Python
```bash
pip install tapirus
# Or use the zero-dependency C-ABI ctypes driver
```

### Node.js / TypeScript
```bash
npm install tapirusdb
```

---

## 2. Python Implementation

Here is a complete, production-ready AI Agent Memory Manager in Python:

```python
import tapirus
import time
from typing import List, Dict, Any

class AgentMemory:
    def __init__(self, vault_path: str = "agent_vault.tapir"):
        # Open local encrypted or plaintext vault (or ":memory:")
        self.conn = tapirus.connect(vault_path)
        self._init_schema()

    def _init_schema(self):
        # 1. Episodic Dialogue & Tool Log
        self.conn.execute("""
            CREATE TABLE IF NOT EXISTS episodes (
                id INTEGER PRIMARY KEY,
                session_id TEXT,
                step INTEGER,
                role TEXT,
                content TEXT,
                embedding VECTOR(4)
            );
        """)

        # 2. Knowledge Graph Associative Relationships
        self.conn.execute("""
            CREATE TABLE IF NOT EXISTS kg_edges (
                src_entity TEXT,
                dst_entity TEXT,
                relation TEXT,
                weight FLOAT
            );
        """)

    def record_episode(self, session_id: str, step: int, role: str, content: str, embedding: List[float]):
        """Records an episodic conversational turn or tool action with embedding."""
        clean_content = content.replace("'", "''")
        self.conn.execute(f"""
            INSERT INTO episodes (id, session_id, step, role, content, embedding)
            VALUES ({int(time.time()*1000)}, '{session_id}', {step}, '{role}', '{clean_content}', {embedding});
        """)

    def get_conversation_flow(self, session_id: str) -> List[Dict[str, Any]]:
        """
        Uses ANSI SQL Window Functions (LAG) to fetch the interaction flow 
        along with what the previous turn said in a single sub-millisecond query.
        """
        sql = f"""
            SELECT 
                step, 
                role, 
                content,
                LAG(content, 1) OVER (PARTITION BY session_id ORDER BY step ASC) as previous_turn
            FROM episodes 
            WHERE session_id = '{session_id}'
            ORDER BY step ASC;
        """
        return self.conn.query(sql)

    def retrieve_similar_episodes(self, query_embedding: List[float], top_k: int = 3) -> List[Dict[str, Any]]:
        """Performs native SIMD Cosine similarity search across all historical episodes."""
        return self.conn.vector_search("episodes", "embedding", query_embedding, top_k=top_k)

    def add_knowledge_link(self, src: str, dst: str, relation: str, weight: float = 1.0):
        """Adds an associative knowledge link between two extracted entities."""
        self.conn.execute(f"""
            INSERT INTO kg_edges VALUES ('{src}', '{dst}', '{relation}', {weight});
        """)

    def detect_topic_clusters(self) -> Dict[str, Any]:
        """Runs native Louvain community modularity algorithm on the agent's knowledge graph."""
        return self.conn.graph_algorithm("louvain")

    def checkpoint(self):
        """Flushes WAL log frames to the main .tapir container file."""
        return self.conn.checkpoint()
```

### Running the Agent Memory

```python
# 1. Instantiate the memory engine
memory = AgentMemory(":memory:")

# 2. Add sample interaction episodes
print("Recording episodic memories...")
memory.record_episode("sess_01", 1, "user", "What is the capital of Malaysia?", [0.1, 0.8, 0.4, 0.2])
memory.record_episode("sess_01", 2, "assistant", "The capital of Malaysia is Kuala Lumpur.", [0.12, 0.79, 0.41, 0.22])
memory.record_episode("sess_01", 3, "user", "What is its population?", [0.05, 0.85, 0.35, 0.18])

# 3. Query interaction flow with SQL Window Functions (LAG)
print("\nEpisodic timeline with window analysis:")
flow = memory.get_conversation_flow("sess_01")
for turn in flow:
    print(f"Step {turn.get('step')}: [{turn.get('role')}] -> {turn.get('content')}")
    if turn.get('previous_turn'):
        print(f"   (Previous context was: '{turn.get('previous_turn')}')")

# 4. Semantic Search via Vector Embeddings
print("\nRetrieving semantic matches:")
query_vector = [0.11, 0.81, 0.39, 0.21]
matches = memory.retrieve_similar_episodes(query_vector, top_k=2)
for m in matches:
    print(f"Match ID: {m.get('id')} (Role: {m.get('role')}) -> {m.get('content')}")

# 5. Extract and link associative knowledge
print("\nClustering associative knowledge graph...")
memory.add_knowledge_link("Malaysia", "Kuala_Lumpur", "HAS_CAPITAL", 1.0)
memory.add_knowledge_link("Kuala_Lumpur", "Petronas_Towers", "LANDMARK", 1.0)
memory.add_knowledge_link("Malaysia", "Putrajaya", "ADMIN_CENTRE", 0.9)

clusters = memory.detect_topic_clusters()
print(f"Discovered Knowledge Communities: {clusters}")
```

---

## 3. Node.js / TypeScript Implementation

```typescript
import { TapirusClient } from "tapirusdb";

async function runAgentMemory() {
  const db = new TapirusClient({ dbPath: "agent_memory.tapir" });

  // 1. Initialize schema
  await db.execute(`
    CREATE TABLE agent_turns (
      id INTEGER PRIMARY KEY,
      turn_index INTEGER,
      speaker TEXT,
      utterance TEXT,
      features VECTOR(4)
    );
  `);

  // 2. Insert interaction with vector
  await db.execute(`
    INSERT INTO agent_turns VALUES (
      1, 1, 'user', 'Analyze quarterly revenue drop', [0.85, 0.12, 0.45, 0.90]
    );
  `);

  // 3. Analytics with SQL Window Function
  const rankedTurns = await db.query(`
    SELECT 
      turn_index, 
      speaker, 
      ROW_NUMBER() OVER (ORDER BY turn_index ASC) as sequential_rank 
    FROM agent_turns;
  `);
  console.log("Ranked agent turns:", rankedTurns);

  // 4. Vector Semantic Retrieval
  const similar = await db.vectorSearch(
    "agent_turns", 
    "features", 
    [0.82, 0.15, 0.40, 0.88], 
    3
  );
  console.log("Vector matches:", similar);

  // 5. Graph Modularity Clustering
  const graphResult = await db.graphAlgorithm("louvain");
  console.log("Topic Clusters:", graphResult);
}

runAgentMemory().catch(console.error);
```

---

## 4. Why This Beats Pinecone + SQLite + Neo4j

| Metric | Traditional RAG Stack | TapirusDB Unified |
|---|---|---|
| **Moving Parts** | 3 databases (Relational + Vector + Graph) | **1 single embedded file** |
| **Query Latency** | 50ms - 250ms (Network roundtrips) | **< 1ms (In-memory C-ABI)** |
| **Monthly Cost** | $150 - $600 / month | **$0.00 Forever** |
| **RAM Footprint** | 500MB - 2GB | **< 4 MB** |
| **Local Privacy** | Data sent to cloud providers | **100% On-Device / Edge** |
| **Crash Safety** | Distributed split-brain risks | **Atomic ARIES WAL2 Checkpoints** |

---

## 5. Production Best Practices

1. **Use `:memory:` for ephemeral agent tasks**: If your agent is processing a single workflow, use an in-memory database to achieve over 150,000 operations per second with zero disk I/O.
2. **Periodic Checkpointing**: Call `conn.checkpoint()` after every session to flush WAL journals back into the main `.tapir` container file.
3. **Encryption at Rest**: If handling sensitive agent tokens or user secrets, supply a passphrase when opening the vault to automatically activate ChaCha20-Poly1305 hardware-accelerated encryption.
