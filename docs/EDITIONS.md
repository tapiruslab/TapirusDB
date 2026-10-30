# Built-in TapirusDB editions

TapirusDB has one native `.tapir` file format. Select a first-party entry point
for the model an application needs; no external database service, plugin or
extension is required. The default `tapirus` CLI and `Connection` Rust API remain
the full Quad engine, with relational SQL, JSON documents, graph and vectors in
one process. AI memory and hybrid retrieval are part of the full engine.

| Entry point | Public operations | Cargo feature | Binary |
|---|---|---|---|
| SQL Lite | Relational SQL only | `lite` | `tapirus-lite` |
| Document | JSON collections only | `document` | `tapirus-document` |
| Graph | Nodes, edges and neighbors only | `graph-only` | `tapirus-graph` |
| Vector | Fixed-dimension vector collections and nearest-neighbor search only | `vector-only` | `tapirus-vector` |
| Quad | All built-in models, including AI memory | default `quad` | `tapirus` |

Build each chosen executable from this source revision:

```sh
cargo build --release -p tapirus --no-default-features --features lite --bin tapirus-lite
cargo build --release -p tapirus --no-default-features --features document --bin tapirus-document
cargo build --release -p tapirus --no-default-features --features graph-only --bin tapirus-graph
cargo build --release -p tapirus --no-default-features --features vector-only --bin tapirus-vector
cargo build --release -p tapirus --bin tapirus
```

These commands build from the checked-out source. Check a published release's
asset list before assuming its edition binaries are available. The release
workflow packages the editions only after its tag build passes verification.

`--no-default-features` matters: otherwise Cargo also activates `quad`. Features
are chosen at build time. To move from Lite to Quad, run the Quad executable
against the same `.tapir` file. Keep a backup before changing an important
database. Adding a feature does not install a runtime module into an existing
executable.

Each first-party binary exposes only its selected model:

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

Rust callers use `tapirus::editions::{LiteDatabase, DocumentDatabase,
GraphDatabase, VectorDatabase}` with the corresponding feature. These wrappers
do not expose the other model APIs. Lite also rejects graph/vector statements,
model-specific SQL functions and direct access to reserved internal tables.
The wrappers do not hide the full `Connection` type from code that deliberately
imports it. `VectorDatabase` requires a fixed positive dimension; its nearest-
neighbor method uses cosine distance. Use the full Quad API for AI memory and
hybrid retrieval.

For a worked build, write, close and reopen flow, see the
[choose-an-edition tutorial](tutorials/choose_an_edition.md).

These editions are **API and executable boundaries**, not a security sandbox.
The shared core still compiles cross-model modules because SQL execution and
storage currently depend on them. A local Windows `min-size` build measured
smaller edition binaries than Quad; the reproducible measurement command is
`python scripts/measure-edition-footprint.py` from the repository root.
RAM savings under equivalent workloads and other platforms remain unproven.
The tagged release workflow is configured to package each
edition separately alongside Quad, with checksums, build metadata and
attestations. These are not published until a new tag completes successfully.
Removing unused engine modules from each build requires further refactoring and
measurement. CI tests each edition build and reopens one file across models.
