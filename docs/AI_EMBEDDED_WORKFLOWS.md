# AI and embedded workflows

These are `1.0.5` source APIs across the Rust core, native Python/Node.js SDKs, `@tapirus/wasm`, `tapirus serve`, and `tapirus mcp`.

## Source lifecycle and multi-tenant isolation

Node uses camelCase requests; Python/Rust/WASM JSON use snake_case. Returned records use snake_case across bindings. For hard multi-tenant owner isolation, wrap any connection with `conn.scoped_memory(MemoryAccessScope::namespace_and_session(ns, sid))` in Rust, `--api-key-namespace <KEY>=<NS>` in `tapirus serve`, or `--namespace` / `--session-id` in `tapirus mcp`.

```python
import tapirus
with tapirus.connect("research.tapir") as db:
    source = {
        "external_id": "manual/chunk-0", "source_version": "manual-r7",
        "content": "Replace the filter after 40 operating hours.",
        "namespace": "robot-7", "tags": ["source:manual"], "ttl_seconds": 86400,
        # Optional actual model vector and pinned model identity:
        # "embedding": vector, "embedding_model": "model-revision/preprocessing-v1",
    }
    source_id = db.upsert_memory(source)
    db.upsert_memory(dict(source, expected_revision=1))
    print(db.search_memories_detailed({
        "query": "filter replacement", "namespace": "robot-7",
        "candidate_limit": 128, "limit": 5,
    }))
    print(db.memory_stats(), db.storage_health())
```

External IDs are unique within namespace/session. Upsert preserves numeric ID,
created timestamp and associations, increments revision and replaces source
fields/vector/model/expiry. Content has a SHA-256 fingerprint. Expected revision
zero requires absence. A stale revision rejects the entire atomic batch.
`store_memory_batch` and `upsert_memory_batch` join SQL transactions/savepoints.

`expires_at` is Unix seconds, mutually exclusive with `ttl_seconds`; expiry uses
the host clock. Reapply TTL on update when desired. Purge expired records to
reclaim their space. `delete_memories` requires explicit selectors or `all: true`.
`except_source_version` removes old chunks within an explicitly selected source;
it cannot be the sole selector. Namespace/tag comparison uses Unicode lowercase;
sessions and external IDs are exact. Filters do not enforce authorization.

## Independent sessions & MVCC snapshot readers

```js
const { TapirusSessionDatabase } = require('tapirus');
const database = new TapirusSessionDatabase('robot.tapir');
const writer = database.session();
const reader = database.session();
writer.transaction(session => {
  session.upsertMemory({ externalId: 'state', content: 'Calibration complete' });
});
console.log(reader.searchMemories({ query: 'calibration' }));
writer.close(); reader.close(); database.close();
```

Python: `database = tapirus.SessionDatabase.open(path)`, then `a = database.session()`. `with a:` commits on success and rolls back on exception while leaving the session open.

Rust: `SessionDatabase::open`, `.session()` / `.session_with_timeout(Duration)`, `.transaction(|session| ...)`, and **`.snapshot_reader()`** (`mvcc_epoch()`). `snapshot_reader()` returns a lock-free committed MVCC snapshot immediately without blocking even when a writer session holds an active uncommitted transaction. Closing an owning writer session automatically rolls back uncommitted work.

## Persistence & resource profiles

AI Memory entries persist in compact bounds-checked binary rows (`TPMEM001` in `__sys_memory`, with transparent JSON read fallback) paired with CRC32-validated persisted HNSW index snapshots (`__sys_memory_index`) for rapid reopen.

| Rust MemoryConfig | Documents | Estimated bytes | Vector dimensions | Candidate maximum | HNSW `ef_search` beam |
|---|---:|---:|---:|---:|---:|
| `default()` | 1,000,000 | 1 GiB | 4096 | 4096 | 512 |
| `edge()` | 10,000 | 16 MiB | 768 | 256 | 32 |
| `research()` | 1,000,000 | 4 GiB on 64-bit | 8192 | 4096 | 1024 |

These profiles enforce memory-admission ceilings before loading persisted sources. Use `MemoryConfig::edge()` for constrained edge hosts or `MemoryConfig::research()` for high-dimensional workstation workloads (`Config { memory: ..., ..Default::default() }`). Default RRF retrieval tuning uses `ef_construction = 128`, `ef_search = 512`, `rrf_k = 20.0`, `vector_weight = 0.65`, and `lexical_weight = 0.25`.
Lower limits below usage
are rejected. `prepareMemoryIndexes` builds indexes before serving traffic.
Reopen rebuilds them from source records. Measure RSS, index build and reopen
time for the deployment. Compact storage and persistent index caches remain
separate optimization work.
The research profile spends more query work to recover high-dimensional
neighbors. A broad namespace filter can expand HNSW traversal up to the
configured maximum; `candidate_limit` bounds retained results, not every graph
node examined. Measure recall against `exact: true` for the intended corpus.

## SQLite migration

The Python wheel installs these commands (also available with
`python -m tapirus_tools.migrate`):

```sh
tapirus-migrate import existing.sqlite imported.tapir --dry-run
tapirus-migrate import existing.sqlite imported.tapir
tapirus-migrate export imported.tapir roundtrip.sqlite
```

Python's standard `sqlite3` reads a consistent read-only snapshot. Source rows
stream through typed bound parameters. The accepted automatic subset is ordinary
tables with INTEGER/REAL/TEXT/BLOB, NOT NULL and one INTEGER PRIMARY KEY.
Unsupported identifiers, views, triggers, indexes/UNIQUE, foreign keys, defaults,
generated columns, CHECK/collation and other types/options are reported and
rejected. SQLite dynamic values that mismatch declared types abort import.
Existing destinations are never overwritten.

Import stages a native file, commits, reopens, checks integrity and compares
row counts plus type-aware SHA-256 multiset fingerprints before atomic
destination creation. That step uses a hard link on the same filesystem; the
filesystem must support it. Native verification currently materializes each
table. Export covers scalar SQL tables, excluding vector/document/graph/memory
models. Hidden SQLite rowids and SQLite semantic equivalence are outside the
contract. Native backup/restore preserves the complete database.

## Runnable document agent

```sh
# No model service required:
tapirus-agent research.tapir ingest manual.txt notes.txt --namespace lab
tapirus-agent research.tapir ask "filter replacement interval" --namespace lab

# An installed local model; supply its real dimension and pinned identity:
tapirus-agent semantic.tapir ingest manual.txt --backend ollama --model YOUR_MODEL --model-id YOUR_PINNED_ID --dimensions 384
tapirus-agent semantic.tapir ask "maintenance interval" --backend ollama --model YOUR_MODEL --model-id YOUR_PINNED_ID --dimensions 384 --chat-model YOUR_CHAT_MODEL

# Verified local multilingual E5 ONNX snapshot, including its manifest:
python scripts/download-eval-model.py --destination scratch/semantic-eval/model
python -m pip install 'onnxruntime>=1.20,<2' 'tokenizers>=0.20,<1' 'numpy>=1.26,<3'
tapirus-agent semantic.tapir ingest manual.txt --backend onnx --model-path scratch/semantic-eval/model --dimensions 384
tapirus-agent semantic.tapir ask "bila ganti penapis?" --backend onnx --model-path scratch/semantic-eval/model --dimensions 384 --retrieval-mode vector
```

Equivalent: `python -m tapirus_tools.agent ...`. Ingestion chunks UTF-8 sources,
generates embeddings before a transaction, upserts stable chunk IDs and atomically
removes stale chunks when sources change/shrink. Source version includes content,
model identity and chunking settings. Retrieval exposes source IDs/text/scores.
Optional generation requests citations; generated answers require evaluation.
The ONNX backend verifies the pinned model files before CPU inference, applies
the E5 `query:` and `passage:` prefixes and uses masked mean pooling with L2
normalization. Model weights are downloaded only by the explicit script above.
Use the same model snapshot for ingestion and queries. The local snapshot is an
evaluation dependency, not part of the TapirusDB distribution.
Long inputs fail by default. `--truncate-model-inputs` explicitly clips local
E5 inputs to 512 tokens, records the count in evaluation output and appends
`/truncate512` to the model identity. Chunk sources when full text matters.

The [Ollama embedding](https://docs.ollama.com/api/embed) and
[chat](https://docs.ollama.com/api/chat) clients use bounded responses, timeouts,
disabled redirects and explicit errors. They never substitute a different model.
`--backend hash` is an explicitly labeled lexical baseline, not a neural model.

Rust `EmbeddingProvider` supplies model identity, dimensions and fallible batches.
`upsert_embedded_memories` validates output before DB mutation. Optional native
`http-embeddings` enables the TLS-capable HTTP provider. Prefer `try_embed_text`
or `embed_documents`; legacy infallible `.embed_text` returns empty on failure.
Credentials require HTTPS; Debug output omits tokens. Pin actual model revisions
and preprocessing; application model labels alone do not prove model equality.

## Retrieval evaluation

```sh
tapirus-eval eval/device-memory-dev-v1.json --output target/retrieval-lexical.json
tapirus-eval eval/device-memory-dev-v1.json --backend hash --output target/retrieval-hash.json
tapirus-eval your-heldout.json --backend ollama --model YOUR_MODEL --model-id YOUR_PINNED_ID --dimensions 384 --output target/retrieval-neural.json
tapirus-eval eval/device-memory-paraphrase-v1.json --backend onnx --model-path scratch/semantic-eval/model --dimensions 384 --retrieval-mode vector --output target/retrieval-onnx-vector.json
python scripts/prepare-beir-scifact.py --source hf
tapirus-eval scratch/semantic-eval/beir/scifact-test.json --backend onnx --model-path scratch/semantic-eval/model --dimensions 384 --truncate-model-inputs --retrieval-mode vector --k 10 --output target/retrieval-scifact-vector.json
cargo bench --bench memory_scale
```

The authored [development corpus](../eval/device-memory-dev-v1.json) has 20
fictional device sources and 40 BM/English queries, including translation pairs.
It is a development fixture, not independent AI evidence or instructions for real
equipment. Use held-out real data before multilingual/semantic claims.
The [paraphrase fixture](../eval/device-memory-paraphrase-v1.json) uses the same
20 fictional sources and 40 BM/English queries phrased without device names.
It was authored before seeing the neural results, but remains an authored
development challenge rather than independent field evidence.
The SciFact preparation script uses the pinned
[BEIR SciFact corpus](https://huggingface.co/datasets/BeIR/scifact) and
[test relevance labels](https://huggingface.co/datasets/BeIR/scifact-qrels).
It verifies SHA-256 for both downloaded revisions and stores the dataset only
under ignored `scratch/`. `--source official` instead checks the archive MD5
published by [BEIR](https://github.com/beir-cellar/beir/wiki/Datasets-available).
The SciFact test split is English scientific claims, not Malay device manuals.

The October 2026 baseline benchmark on `intfloat/multilingual-e5-small` (revision `614241f622f53c4eeff9890bdc4f31cfecc418b3`, 40 queries) informed the `v1.0.5` retrieval tuning (`ef_construction = 128`, `ef_search = 512`, `rrf_k = 20.0`, `vector_weight = 0.65`, `lexical_weight = 0.25`):

| Fixture | Retrieval Mode | Recall@5 | MRR | nDCG@5 |
|---|---|---:|---:|---:|
| Named device queries | Lexical | 97.5% | 0.797 | 0.842 |
| Named device queries | E5 hybrid | 100% | 0.865 | 0.899 |
| Paraphrases | Lexical | 47.5% | 0.359 | 0.388 |
| Paraphrases | E5 vector | 80.0% | 0.656 | 0.692 |

The vector row uses actual ONNX inference in the installed Python wheel. The
other E5 rows use vectors generated by that same pinned local model. Results
and per-query misses are in `audit/bukti_2026-10-17`. This is retrieval quality,
not a measured accuracy for generated answers or other subject matter.

Reports include Recall@k/MRR/nDCG, per-language misses, latency percentiles,
ingestion/index/reopen times, observed peak process RSS, disk bytes and
dataset/model fingerprints. RSS includes Python and the engine. One short query
pass does not establish a stable latency distribution. Models are not downloaded
automatically. External embeddings use `--embeddings vectors.json` with
`{"model_id":"pinned-v1","dimensions":2,"documents":{"doc-id":[1,0]},"queries":{"query-id":[1,0]}}`.

The persisted memory scale benchmark compares ANN to exact neighbors of
independent deterministic random queries. Set `TAPIRUS_BENCH_DOCS`,
`TAPIRUS_BENCH_DIMS`, `TAPIRUS_BENCH_QUERIES`, `TAPIRUS_BENCH_EF` and
`TAPIRUS_BENCH_OUTPUT`. Synthetic ANN recall measures neighbor recovery, not
semantic relevance. Avoid competing builds/workloads for timing measurements.

## Per-device boundary and durability

Native TapirusDB requires `std` and an OS/filesystem. Browser WASM uses snapshots.
The separate [Tapirus Micro](../crates/tapirus-micro/README.md) component builds
for bare-metal Cortex-M/RISC-V: bounded flash records, no allocator and an `i8`
dot product. It has its own format and no SQL/HNSW/LLM. Target compilation and
simulation do not qualify a physical board or safety-critical machine.

Storage fault tests cover partial frames, write/flush/sync errors, torn base
headers, WAL reset failures, backup preservation and active-target restore
refusal. Native backup stages/syncs before replacement. `storage_health` exposes
deferred auto-checkpoint errors and whether reopen is required. A durable WAL
commit remains successful when subsequent automatic maintenance fails.
Run `cargo test --features fault-injection --test io_fault_recovery_tests`;
production default builds do not expose fault controls.
