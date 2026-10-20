# Choose a TapirusDB edition

This tutorial uses the current source tree, not an already-published release
archive. Pick the entry point your application needs. All editions use the
native `.tapir` format and run without another database service.

| If you need | Build | Public entry point |
|---|---|---|
| Relational SQL only | Lite | `tapirus-lite` / `LiteDatabase` |
| JSON collections only | Document | `tapirus-document` / `DocumentDatabase` |
| Nodes and edges only | Graph | `tapirus-graph` / `GraphDatabase` |
| Embeddings only | Vector | `tapirus-vector` / `VectorDatabase` |
| SQL + document + graph + vector + AI memory | Quad | `tapirus` / `Connection` |

## 1. Build a SQL-only binary

From the repository root:

```sh
cargo build --release -p tapirus --no-default-features --features lite --bin tapirus-lite
```

On Windows PowerShell, run:

```powershell
./target/release/tapirus-lite.exe demo.tapir --exec "CREATE TABLE notes (id INTEGER PRIMARY KEY, body TEXT)"
./target/release/tapirus-lite.exe demo.tapir --exec "INSERT INTO notes (id, body) VALUES (1, 'local note')"
./target/release/tapirus-lite.exe demo.tapir --query "SELECT id, body FROM notes"
```

On Linux or macOS, use `./target/release/tapirus-lite` with the same arguments.
The CLI opens and closes the file for each command. Lite rejects graph/vector
syntax and direct SQL access to reserved internal tables.

## 2. Add a document through the document API

Build the document binary with:

```sh
cargo build --release -p tapirus --no-default-features --features document --bin tapirus-document
```

Rust callers can use the `DocumentDatabase` wrapper (enable the `document`
feature and disable default features in the application's Cargo dependency):

```rust
use serde_json::json;
use tapirus::editions::DocumentDatabase;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let db = DocumentDatabase::open("demo.tapir")?;
    let people = db.collection("people")?;
    let id = people.insert_one(&json!({"name": "Alex Chen"}))?;
    println!("document id: {id}");
    drop(people);
    db.close()?;
    Ok(())
}
```

Close one process before reopening the same file from another process. Take a
backup of important data before changing builds or running migrations.

## 3. Reopen the same file with Quad

Build the default binary or use the full Rust `Connection` API:

```sh
cargo build --release -p tapirus --bin tapirus
```

```rust
use tapirus::Connection;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let db = Connection::open("demo.tapir")?;
    println!("{:?}", db.query("SELECT id, body FROM notes")?);
    println!("{:?}", db.collection("people")?.find_all()?);
    db.close()?;
    Ok(())
}
```

Graph and Vector have their own builds and APIs in the [edition reference](../EDITIONS.md).
The cross-edition test opens one file sequentially through all four editions,
then reads each model through Quad (`tests/edition_profiles.rs`).

## Limits to understand

An edition restricts its wrapper and CLI, not arbitrary code that imports the
full `Connection` API. The core still compiles some cross-model code; smaller
RAM use and security isolation are not proven. Lite supports a SQL subset, not
SQLite drop-in compatibility. No edition is a substitute for target-platform
recovery and performance testing.
