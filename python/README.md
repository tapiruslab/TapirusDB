# TapirusDB Python Client

> **"One Engine. Four Models. Zero Data Sprawl."**  
> The Safe-Rust In-Process AI Multi-Model Database Engine (`tapirus`).  
> *Architected by **Ahmad Faiz***

[![License: BSL-1.1](https://img.shields.io/badge/License-BSL--1.1-blue.svg)](https://github.com/tapiruslab/TapirusDB/blob/master/LICENSE)
[![PyPI version](https://img.shields.io/badge/pypi-0.1.2-brightgreen.svg)](https://pypi.org/project/tapirus/)
[![Rust](https://img.shields.io/badge/Pure_Safe_Rust-100%25-orange.svg)](https://www.rust-lang.org/)

## Features
- **Zero Daemons / In-Process:** Runs directly inside your Python runtime as a native in-process engine without external server processes.
- **Ultra-Low Memory:** Idle memory `< 4 MB RAM`.
- **Quad-Model Architecture:** SQL relational tables, JSON documents, native HNSW vector search, and Property Graph with sub-microsecond GraphRAG.
- **Single-File Durability:** All models persisted in a single `.tapir` file with crash-resilient WAL.
- **Native Cryptography:** Page-level ChaCha20-Poly1305 authenticated encryption (AEAD).
- **AI Agent Memory & MCP:** Seamless integration with LLMs (Claude, GPT-4o) and edge SLMs (Phi-3, Gemma-2).

## Installation

```bash
# Install directly from local repository:
pip install .

# Or from PyPI (when published):
pip install tapirus
```

## Quick Start

### Embedded Relational SQL & ACID Transactions
```python
from tapirus import Tapirus

# Open or create single-file database
with Tapirus.open("app.tapir") as db:
    # 1. Create tables
    db.execute("""
        CREATE TABLE rovers (
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            battery REAL
        );
    """)

    # 2. Insert records
    db.execute("INSERT INTO rovers (id, name, battery) VALUES (1, 'Curiosity', 98.5);")

    # 3. Query records as Python dictionaries
    rows = db.query("SELECT id, name, battery FROM rovers WHERE id = 1;")
    print(rows)  # [{'id': 1, 'name': 'Curiosity', 'battery': 98.5}]

    # 4. Atomic transactions
    db.execute("BEGIN;")
    db.execute("UPDATE rovers SET battery = 95.0 WHERE id = 1;")
    db.execute("COMMIT;")
```

### Encrypted Database at Rest (ChaCha20-Poly1305)
```python
from tapirus import Tapirus

# Open encrypted database with passphrase
with Tapirus.open("vault.tapir", passphrase="ultra_secure_passphrase") as db:
    db.execute("CREATE TABLE secrets (id INT PRIMARY KEY, token TEXT);")
    db.execute("INSERT INTO secrets VALUES (1, 'sk-live-confidential-credential');")
```

## License
Business Source License 1.1 (BSL-1.1). Commercial use permitted under the terms defined in the main [TapirusDB repository](https://github.com/tapiruslab/TapirusDB).
