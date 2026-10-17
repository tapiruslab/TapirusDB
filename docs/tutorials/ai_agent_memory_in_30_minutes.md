# Agent Memory Quickstart with TapirusDB

> This guide shows the Python SDK's built-in text memory API. It does not claim a latency, RAM ceiling, retrieval accuracy, or crash-safety level. Check the API and limitations for the package version you install.

TapirusDB can store text memories in a local database file and retrieve them with the Python SDK. This is a small starting point for experimenting with agent context; evaluate the retrieval behavior and persistence path for your own workload.

## Install

```bash
pip install tapirus
```

## Store and recall text memories

```python
import tapirus

with tapirus.connect("agent_memory.tapir") as db:
    db.remember("Alex prefers concise technical answers.")
    db.remember("The current project uses Rust and PostgreSQL.")

    matches = db.recall("What style of answer does Alex prefer?", limit=3)
    for item in matches:
        print(item.get("content", ""))

    # Optional: format the returned text as context for an application prompt.
    prompt_context = db.recall_prompt("Alex's answer preferences", limit=3)
    print(prompt_context)

    db.checkpoint()
```

The current Python implementation performs text matching against stored content and falls back to recent rows when no match is found. It is not an embedding-based semantic search API, and its result should be reviewed before it is used to inform an agent response.

## Rust API

The Rust `Connection` API also provides `remember`, `recall`, and `recall_prompt` methods:

```rust
use tapirus::Connection;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let db = Connection::open("agent_memory.tapir")?;
    db.remember("Alex prefers concise technical answers.")?;

    for item in db.recall("answer preferences", 3) {
        println!("{}", item.entry.content);
    }
    println!("{}", db.recall_prompt("answer preferences", 3));
    db.checkpoint()?;
    Ok(())
}
```

## Before using this in production

- Review the package's supported SQL and SDK methods for the version you ship.
- Test write, close, reopen, and read behavior on the target platform.
- Measure memory and latency with your dataset and application runtime.
- Avoid storing secrets unless you have reviewed the optional encryption configuration and key handling.
- Treat recalled text as untrusted input and apply your application's prompt-injection controls.
