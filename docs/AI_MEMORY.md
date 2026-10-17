# Transactional AI document memory

TapirusDB stores document text, application-provided embeddings, source tags and namespace/session fields in the database. Retrieval combines BM25 keyword matching, cosine similarity, recency and importance. Every result includes its original source ID and text plus separate score components.

The `storeMemory` / `searchMemories` APIs use the Rust engine. Applications can supply embeddings from their chosen model; omitting embeddings enables lexical retrieval. These methods make no model-service or network calls. Search uses BM25 and lazy HNSW indexes per model/dimension. See [AI and embedded workflows](AI_EMBEDDED_WORKFLOWS.md) for lifecycle, sessions, limits, SQLite migration and a runnable agent.

## Node.js

```js
const { open } = require('tapirus');
const db = open('assistant.tapir');

db.execute('BEGIN;');
const id = db.storeMemory({
  content: 'The maintenance interval is 500 operating hours.',
  embedding: [1, 0], // Example only: use an embedding from your model.
  namespace: 'manuals',
  sessionId: 'workshop',
  tags: ['source:maintenance-manual'],
  importance: 0.8,
});
db.execute('COMMIT;');

const sources = db.searchMemories({
  query: 'maintenance interval',
  embedding: [1, 0], // Same embedding model and dimensions as the documents.
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

db.forgetMemory(id); // Propagates storage errors.
db.close();
```

`BEGIN`, `COMMIT`, `ROLLBACK` and SQL savepoints cover both SQL changes and document-memory changes. An individual memory operation is atomic in autocommit mode. Store, link, checked deletion and checked pruning propagate storage errors.

## Python

The native [Python SDK](../sdks/python/README.md) exposes the same request fields in snake case:

```python
import tapirus

with tapirus.connect("assistant.tapir") as db:
    db.store_memory({
        "content": "The maintenance interval is 500 operating hours.",
        "embedding": [1.0, 0.0],
        "namespace": "manuals",
        "session_id": "workshop",
    })
    print(db.search_memories({"query": "maintenance interval", "namespace": "manuals"}))
```

Requires Python 3.10+. SQL parameters are bound through `db.execute(sql, params)` and `db.query(sql, params)`. The context manager closes the file; explicit transactions must be committed explicitly.

## Rust

```rust
use tapirus::{Connection, MemoryDocument, MemorySearch, Result};

fn main() -> Result<()> {
    let db = Connection::open("assistant.tapir")?;
    db.store_memory(&MemoryDocument {
        content: "The maintenance interval is 500 operating hours.".into(),
        embedding: Some(vec![1.0, 0.0]),
        namespace: Some("manuals".into()),
        ..Default::default()
    })?;
    let sources = db.search_memories(&MemorySearch {
        query: Some("maintenance interval".into()),
        embedding: Some(vec![1.0, 0.0]),
        namespace: Some("manuals".into()),
        ..Default::default()
    })?;
    println!("{sources:?}");
    db.close()
}
```

## Retrieval contract

- `semantic_score` is positive cosine similarity, clipped at zero. Vector retrieval selects matching model identity and dimension; a named model with mismatched dimensions is rejected. Unnamed legacy vectors are a separate space.
- `lexical_score` is the BM25 score produced by the index. It is not a probability or constrained to one.
- `recency_score` is the temporal decay component. `combined_score` is bounded to zero through one; it is a ranking score, not confidence. Default fusion is reciprocal rank fusion; `fusion: 'weighted'` uses bounded channel scores.
- Namespace and session fields filter sources. They are application filters, not authentication; enforce user access in the application. Namespace comparison is case insensitive and session comparison is exact.
- Scores depend on the corpus, query and embedding model. Use a labeled retrieval evaluation for your workload before choosing thresholds.
- `candidateLimit` (`candidate_limit` in Python/Rust) bounds retained candidates per channel. At most twice that count reaches final scoring; lexical postings and HNSW traversal can examine more records. Small selective vector scopes use exact scoring. Explicit `exact: true` scans vectors for ground truth. Combined scope filters may construct an intersection proportional to the smallest posting list.
- Expired sources are excluded before candidate retention; purge them to reclaim space. Access counters are runtime observations, not separately durable writes for every read.
- Legacy `recall`, `memory_recall` and `memory_recall_rrf` retain their older scan/graph behavior. Use `searchMemories` / `search_memories` for the indexed path.
- `memory_forget_checked` and `memory_prune_at_checked` are the fallible Rust APIs. Legacy boolean/count wrappers return `false`/zero on failure; use the checked APIs when error reporting matters.

## Browser workspace

The [WASM workspace](../crates/tapirus-wasm/demo/index.html) runs the same Rust engine, restores committed snapshots from IndexedDB and opens/downloads native `.tapir` files. Browser copies are unencrypted. Keep downloaded backups and use the native SDK for encrypted files or larger workloads.

Build and verify from the repository root:

```sh
rustup target add wasm32-unknown-unknown
# Install wasm-bindgen-cli at the exact version in Cargo.lock.
node scripts/build-wasm.js
node scripts/build-node-sdk.js --debug
node scripts/test-wasm.mjs
node scripts/test-browser-workspace.mjs
```

Set `BROWSER_BIN` when Chrome/Edge is not at the smoke test's default path. Serve the repository over HTTP(S) to open the workspace; ES modules cannot be loaded by double-clicking a file URL. `node scripts/package-browser-workspace.js` prepares the standalone workspace in Studio's `public/native` directory.
