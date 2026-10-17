# TapirusDB API Cookbook

This cookbook keeps only a minimal Rust example whose API is shown in the current root README. The former recipes included unsupported SQL shapes, unverified resource and latency figures, and claims of broad maintenance or transaction guarantees; those examples have been withdrawn.

## Scope

TapirusDB exposes a Rust database API and a documented SQL subset. Check the parser and executor for supported query forms. This example does not certify crash recovery or general ACID behavior. Use disposable data until the exact workload has been tested through close, reopen, and recovery.

## Open, write, query, and close

```rust
use tapirus::{Connection, Result};

fn main() -> Result<()> {
    let db = Connection::open("notes.tapir")?;
    db.execute("CREATE TABLE IF NOT EXISTS notes (id INTEGER PRIMARY KEY, body TEXT NOT NULL)")?;
    db.execute("INSERT INTO notes (id, body) VALUES (1, 'Review the supported API')")?;
    let rows = db.query("SELECT id, body FROM notes")?;
    println!("{rows:?}");
    db.close()?;
    Ok(())
}
```

Use parameterized APIs for user-provided values. Do not assume SQL syntax or transaction behavior beyond what is implemented and covered by tests.

## Further reading

- [Project scope and build commands](../README.md)
- [Storage and WAL implementation notes](architecture/01_STORAGE_ENGINE_AND_PAGING.md)
- [Transaction and recovery notes](architecture/02_WAL_TRANSACTIONS_AND_RECOVERY.md)
- [Release validation and regression evidence](RELEASE_VALIDATION.md)
