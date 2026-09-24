# 🦛 TapirusDB for Rust Developers

Zero-overhead, in-process multi-model database engine in 100% Safe Rust (`#![forbid(unsafe_code)]`).

## 📦 Adding Dependency

Add `tapirus` to your `Cargo.toml`:

```toml
[dependencies]
tapirus = "0.1.2"
serde_json = "1.0"
```

## 🚀 Quick Example

```rust
use tapirus::vector::DistanceMetric;
use tapirus::{Connection, Result};
use serde_json::json;

fn main() -> Result<()> {
    // 1. Open or create single-file database
    let db = Connection::open("app.tapir")?;

    // 2. Relational SQL + Vector Column
    db.execute("
        CREATE TABLE knowledge (
            id INTEGER PRIMARY KEY,
            title TEXT NOT NULL,
            embedding VECTOR(4)
        );
    ")?;

    db.execute("
        INSERT INTO knowledge (id, title, embedding)
        VALUES (1, 'Safe Rust Systems', [0.95, 0.05, 0.0, 0.0]);
    ")?;

    // 3. Schema-less Document Collection (MongoDB-style)
    let users = db.collection("users")?;
    let doc_id = users.insert_one(&json!({
        "name": "Faiz",
        "company": "Tapirus Tech Lab",
        "role": "Systems Architect"
    }))?;

    // 4. Native Knowledge Graph (GraphRAG)
    db.graph_add_node(1, "Patient", r#"{"name":"Alice"}"#)?;
    db.graph_add_node(2, "Medicine", r#"{"name":"Aspirin"}"#)?;
    db.graph_add_edge(1, 2, "TREATS", 1.0, "")?;

    // 5. Sub-Microsecond Chaining Pipeline (Graph-to-Vector Native Traversal)
    let matches = db
        .chain(1)
        .out(Some("TREATS"))
        .filter_label("Medicine")
        .vector_near(&[0.90, 0.10, 0.0, 0.0], 5, DistanceMetric::Cosine)?;

    for m in matches {
        println!("Match: {} (score: {:.4})", m.node.label, m.score);
    }

    Ok(())
}
```

## 🏃 Running the Example

```bash
cargo run
```
