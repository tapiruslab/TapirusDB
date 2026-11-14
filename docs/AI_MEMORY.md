# Transactional AI Document Memory

TapirusDB stores document text, caller- or provider-supplied embeddings, tags, TTLs, content hashes, and namespace/session metadata inside the same ACID database transaction as relational SQL tables. Hybrid retrieval combines **HNSW vector cosine similarity**, **BM25 lexical scoring**, **temporal recency decay**, and **importance** through Reciprocal Rank Fusion (`rrf_k = 20.0`, `vector_weight = 0.65`, `lexical_weight = 0.25`) or bounded weighted fusion.

AI Memory records persist in compact binary `TPMEM001` rows (`__sys_memory`) alongside CRC32-validated HNSW index snapshots (`__sys_memory_index`) for rapid reopen without full graph rebuilds.

See [AI and embedded workflows](AI_EMBEDDED_WORKFLOWS.md) for revision-checked upserts, atomic batches, `MemoryAccessScope`, MVCC snapshot readers, SQLite migration, and neural ONNX/Ollama/OpenAI embedding integrations.

## Node.js

```js
const { open } = require('tapirus');
const db = open('assistant.tapir');

db.execute('BEGIN;');
const id = db.storeMemory({
  content: 'The maintenance interval is 500 operating hours.',
  embedding: [1, 0], // Supply an embedding vector from your chosen model.
  namespace: 'manuals',
  sessionId: 'workshop',
  tags: ['source:maintenance-manual'],
  importance: 0.8,
});
db.execute('COMMIT;');

const sources = db.searchMemories({
  query: 'maintenance interval',
  embedding: [1, 0],
  namespace: 'manuals',
  sessionId: 'workshop',
  limit: 5,
});
console.log(sources.map(result => ({
  sourceId: result.entry.id,
  content: result.entry.content,
  lexical: result.lexical_score,
  cosine: result.semantic_score,
})));

db.forgetMemory(id);
db.close();
```

`BEGIN`, `COMMIT`, `ROLLBACK`, and SQL savepoints cover SQL and document-memory mutations atomically.

## Python

The native [Python SDK](../sdks/python/README.md) exposes the full API with optional passphrase encryption and `SessionDatabase`:

```python
import tapirus

with tapirus.connect("assistant.tapir", "strong-passphrase") as db:
    db.store_memory({
        "content": "The maintenance interval is 500 operating hours.",
        "embedding": [1.0, 0.0],
        "namespace": "manuals",
        "session_id": "workshop",
    })
    print(db.search_memories({"query": "maintenance interval", "namespace": "manuals"}))
```

## Rust (with Hard Per-Owner Scope Isolation)

```rust
use tapirus::{Connection, MemoryAccessScope, MemoryDocument, MemorySearch, Result};

fn main() -> Result<()> {
    let conn = Connection::open("assistant.tapir")?;
    let scoped = conn.scoped_memory(MemoryAccessScope::namespace("manuals"))?;

    scoped.store_memory(&MemoryDocument {
        content: "The maintenance interval is 500 operating hours.".into(),
        embedding: Some(vec![1.0, 0.0]),
        ..Default::default()
    })?;
    let sources = scoped.search_memories(&MemorySearch {
        query: Some("maintenance interval".into()),
        embedding: Some(vec![1.0, 0.0]),
        ..Default::default()
    })?;
    println!("{sources:?}");
    conn.close()
}
```

## Retrieval & Isolation Architecture

- **Multi-Model Vector Spaces:** `semantic_score` uses cosine similarity matched strictly by model identity and vector dimensionality.
- **Per-Owner Security Scoping:** Use `Connection::scoped_memory(MemoryAccessScope)` in Rust, `--api-key-namespace <KEY>=<NS>` on `tapirus serve`, or `--namespace` / `--session-id` on `tapirus mcp` to enforce hard multi-tenant owner boundaries (`Error::PermissionDenied` on cross-scope access and `__sys_memory` SQL isolation).
- **Default Search & Tuning:** `MemoryConfig::default()` configures `ef_construction = 128`, `ef_search = 512`, `candidate_limit = 128`, `rrf_k = 20.0`, `vector_weight = 0.65`, and `lexical_weight = 0.25`. Exact vector scans (`exact: true`) and small-scope exact scoring remain built-in.
- **Checked Fallible Rust APIs:** `memory_forget_checked` and `memory_prune_at_checked` propagate storage I/O errors directly.

## Browser & WASM Runtime

`@tapirus/wasm` and the [WASM workspace](../crates/tapirus-wasm/demo/index.html) execute the same Safe-Rust engine inside WebAssembly, persisting committed snapshots to IndexedDB and importing/exporting portable `.tapir` files. Passphrase-encrypted in-browser vaults use `TapirusWasm.new_encrypted(passphrase)` and `TapirusWasm.import_encrypted_bytes(bytes, passphrase)`.

```sh
rustup target add wasm32-unknown-unknown
node scripts/build-wasm.js
node scripts/test-wasm.mjs
node scripts/test-browser-workspace.mjs
```
