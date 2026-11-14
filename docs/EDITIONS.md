# Built-in TapirusDB Editions

TapirusDB uses a single interoperable `.tapir` file format across all editions. Deploy the full Quad engine (`tapirus`) or select a dedicated single-model binary and Rust facade (`lite`, `document`, `graph-only`, `vector-only`) cleanly at build time.

| Entry point | Model Scope | Cargo feature | Binary | Rust Facade |
|---|---|---|---|---|
| **Quad (Default)** | SQL + Vector + Graph + Document + AI Memory | default (`quad`) | `tapirus` | `tapirus::Connection` / `SessionDatabase` |
| **SQL Lite** | Relational SQL + SQLite compatibility features | `lite` | `tapirus-lite` | `tapirus::editions::LiteDatabase` |
| **Document** | JSON document collections | `document` | `tapirus-document` | `tapirus::editions::DocumentDatabase` |
| **Graph** | Property nodes, weighted edges, neighbors & traversal | `graph-only` | `tapirus-graph` | `tapirus::editions::GraphDatabase` |
| **Vector** | Fixed-dimension vector collections & cosine HNSW search | `vector-only` | `tapirus-vector` | `tapirus::editions::VectorDatabase` |

## Build Commands

```sh
cargo build --release -p tapirus --no-default-features --features lite --bin tapirus-lite
cargo build --release -p tapirus --no-default-features --features document --bin tapirus-document
cargo build --release -p tapirus --no-default-features --features graph-only --bin tapirus-graph
cargo build --release -p tapirus --no-default-features --features vector-only --bin tapirus-vector
cargo build --release -p tapirus --bin tapirus
```

Pass `--no-default-features` when compiling a single-model build; otherwise Cargo activates `quad`. Because every edition shares the `.tapir` format, databases created by `tapirus-lite` open seamlessly in the full `tapirus` engine when AI memory, graph, or vector capabilities are needed later.

## CLI Quick Usage

```sh
tapirus-lite app.tapir --exec "CREATE TABLE notes (id INTEGER PRIMARY KEY, text TEXT)"
tapirus-lite app.tapir --query "SELECT id, text FROM notes"
tapirus-document app.tapir people insert '{"name":"Ada"}'
tapirus-document app.tapir people list
tapirus-graph app.tapir node 1 Person '{"name":"Ada"}'
tapirus-graph app.tapir neighbors 1
tapirus-vector app.tapir people 2 insert 1 '[1.0,0.0]' '{"name":"Ada"}'
tapirus-vector app.tapir people 2 search '[1.0,0.0]' 5
```

- `LiteDatabase` enforces strict SQL-only entry boundaries and blocks internal `__sys_*` tables.
- All editions share CI compilation, cross-edition `.tapir` interchange tests, and footprint tracking via `python scripts/measure-edition-footprint.py`.
- For bare-metal flash deployments (`no_std`, zero-alloc `TPMIC001` slots), use [`tapirus-micro`](../crates/tapirus-micro/README.md) with offline capsule bridging via [`tapirus-iot`](IOT_CAPSULE.md).

See [Choose an edition tutorial](tutorials/choose_an_edition.md) for a complete walkthrough.
