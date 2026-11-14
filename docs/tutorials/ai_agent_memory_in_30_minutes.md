# Agent Memory Quickstart with TapirusDB

Store agent context, user preferences, and source citations inside a single ACID `.tapir` database using either TapirusDB's lightweight text memory helpers or indexed hybrid HNSW + BM25 retrieval (`store_memory` / `search_memories`).

## Install

```bash
pip install tapirus
```

## 1. Hybrid AI Memory (`store_memory` & `search_memories`)

Store document sources with namespace/session tags and optional model-generated embeddings for HNSW cosine + BM25 RRF retrieval (`ef_construction = 128`, `ef_search = 512`, `rrf_k = 20.0`):

```python
import tapirus

with tapirus.connect("agent_memory.tapir", "strong-passphrase") as db:
    db.store_memory({
        "content": "Alex prefers concise Rust and PostgreSQL architecture explanations.",
        "namespace": "user-prefs",
        "session_id": "session-1",
        "tags": ["preference"],
        # Optional: pass embedding=[...] from your local ONNX, Ollama, or OpenAI model
    })

    hits = db.search_memories({
        "query": "concise technical style",
        "namespace": "user-prefs",
        "limit": 3,
    })
    for hit in hits:
        print(hit["entry"]["content"], "score:", hit["combined_score"])
```

For multi-step pipelines with chunking and local multilingual E5 ONNX semantic embeddings, see `tapirus-agent` and [AI and embedded workflows](../AI_EMBEDDED_WORKFLOWS.md).

## 2. Quick Conversational Memory (`remember` & `recall`)

```python
import tapirus

with tapirus.connect("quick_memory.tapir") as db:
    db.remember("Alex prefers concise technical answers.")
    db.remember("The current project uses Rust and TapirusDB.")

    matches = db.recall("What style of answer does Alex prefer?", limit=3)
    for item in matches:
        print(item.get("content", ""))

    prompt_context = db.recall_prompt("Alex's answer preferences", limit=3)
    print(prompt_context)

    db.checkpoint()
```

## 3. Rust API (with Multi-Tenant Scope Isolation)

```rust
use tapirus::{Connection, MemoryAccessScope, MemoryDocument, MemorySearch, Result};

fn main() -> Result<()> {
    let conn = Connection::open("agent_memory.tapir")?;
    let scoped = conn.scoped_memory(MemoryAccessScope::namespace("user-prefs"))?;

    scoped.store_memory(&MemoryDocument {
        content: "Alex prefers concise technical answers.".into(),
        ..Default::default()
    })?;
    let hits = scoped.search_memories(&MemorySearch {
        query: Some("technical style".into()),
        limit: 3,
        ..Default::default()
    })?;
    println!("Retrieved {} scoped memories", hits.len());
    conn.close()
}
```
