# TapirusDB API Cookbook

Practical, verified examples covering encrypted MVCC transactions, relational SQLite compatibility features, HNSW vector search, GraphRAG traversals, and scoped AI document memory.

---

## 1. Encrypted `SessionDatabase` with Non-Blocking MVCC Readers (Rust)

```rust
use tapirus::{Config, Result, SessionDatabase, Value};

fn main() -> Result<()> {
    let db = SessionDatabase::open_with_config(
        "app.tapir",
        Config {
            encryption_key: Some("strong-passphrase".into()),
            concurrent_access: true,
            ..Default::default()
        },
    )?;

    let writer = db.session()?;
    writer.transaction(|tx| {
        tx.execute(
            "CREATE TABLE IF NOT EXISTS projects (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                status TEXT DEFAULT 'active'
            )",
        )?;
        tx.execute_with_params(
            "INSERT INTO projects (id, name) VALUES (?, ?)
             ON CONFLICT DO UPDATE SET name = EXCLUDED.name",
            &[Value::Integer(1), Value::Text("Tapirus Core".into())],
        )?;
        Ok(())
    })?;

    // Acquire a committed snapshot reader without blocking active writers
    let reader = db.snapshot_reader()?;
    let rows = reader.query_with_params(
        "SELECT id, name, status FROM projects WHERE id = ?",
        &[Value::Integer(1)],
    )?;
    println!("Committed epoch {}: {:?}", reader.epoch(), rows);

    writer.close()?;
    db.close()?;
    Ok(())
}
```

---

## 2. Relational Constraints, Triggers & FTS5 Full-Text Search

```rust
use tapirus::{Connection, Result};

fn main() -> Result<()> {
    let db = Connection::open_in_memory()?;

    db.execute(
        "CREATE TABLE departments (
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL
        )",
    )?;
    db.execute(
        "CREATE TABLE employees (
            id INTEGER PRIMARY KEY,
            dept_id INTEGER NOT NULL REFERENCES departments(id) ON DELETE CASCADE,
            age INTEGER NOT NULL CHECK (age >= 18),
            role TEXT DEFAULT 'engineer'
        )",
    )?;
    db.execute("CREATE INDEX idx_emp_dept_role ON employees (dept_id, role)")?;

    db.execute("CREATE VIRTUAL TABLE docs_fts USING fts5(title, body)")?;
    db.execute("INSERT INTO docs_fts (title, body) VALUES ('WAL Guide', 'Safe-Rust crash recovery and MVCC snapshots')")?;
    let hits = db.query("SELECT title FROM docs_fts WHERE docs_fts MATCH 'recovery'")?;
    println!("FTS5 hits: {hits:?}");

    db.close()?;
    Ok(())
}
```

---

## 3. Multi-Tenant Scoped AI Memory (`upsert_memory` + Hybrid Search)

```rust
use tapirus::{Connection, MemoryAccessScope, MemoryDocument, MemorySearch, Result};

fn main() -> Result<()> {
    let raw = Connection::open("memory.tapir")?;
    let tenant_a = raw.scoped_memory(MemoryAccessScope::namespace_and_session(
        "acme-corp",
        "session-42",
    ))?;

    let id = tenant_a.upsert_memory(&MemoryDocument {
        external_id: Some("manual/chunk-0".into()),
        source_version: Some("v2.1".into()),
        content: "Inspect coolant valves after every 500 operating hours.".into(),
        tags: vec!["source:maintenance".into()],
        ttl_seconds: Some(86_400),
        ..Default::default()
    })?;

    let results = tenant_a.search_memories(&MemorySearch {
        query: Some("coolant maintenance interval".into()),
        limit: 5,
        ..Default::default()
    })?;
    println!("Stored ID {id}, retrieved {} citation(s)", results.len());

    raw.close()?;
    Ok(())
}
```

---

## 4. Node.js, Python & Encrypted WASM Snapshots

- **Node.js (`sdks/nodejs`):** See [Node.js SDK README](../sdks/nodejs/README.md) for `open`, `TapirusSessionDatabase`, CDC `subscribe`, and `storeMemory`/`searchMemories`.
- **Python (`sdks/python`):** See [Python SDK README](../sdks/python/README.md) for `tapirus.connect(path, passphrase)`, `SessionDatabase`, `upsert_memory`, and `tapirus-agent` ONNX semantic retrieval.
- **Browser / WASM (`@tapirus/wasm`):**

```js
import init, { TapirusWasm } from '@tapirus/wasm';

await init();
const vault = TapirusWasm.new_encrypted('browser-passphrase');
vault.execute('CREATE TABLE secrets (id INTEGER PRIMARY KEY, note TEXT NOT NULL)');
vault.execute_params('INSERT INTO secrets VALUES (?, ?)', JSON.stringify([1, 'Encrypted in WASM']));

const encryptedSnapshot = vault.export_bytes();
const reopened = TapirusWasm.import_encrypted_bytes(encryptedSnapshot, 'browser-passphrase');
console.log(JSON.parse(reopened.query_params_json('SELECT * FROM secrets WHERE id = ?', '[1]')));
```

---

## Further Reading

- [Root README](../README.md)
- [Storage & Paging Architecture](architecture/01_STORAGE_ENGINE_AND_PAGING.md)
- [WAL, MVCC & Recovery Architecture](architecture/02_WAL_TRANSACTIONS_AND_RECOVERY.md)
- [AI & Embedded Workflows](AI_EMBEDDED_WORKFLOWS.md)
- [Release Validation](RELEASE_VALIDATION.md)
