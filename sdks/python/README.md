<div align="center">

<img src="https://raw.githubusercontent.com/tapiruslab/TapirusDB/main/assets/icons/python.svg" width="64" height="64" alt="Python Logo" />

# TapirusDB Python SDK (`tapirus`)

### Official Python Client for TapirusDB — Embedded Quad-Model AI Database & Memory Engine
**Relational SQL • HNSW Vector Search • openCypher Knowledge Graph • JSON Documents**  
*Single Encrypted `.tapir` Container • 100% Safe Rust Core • < 4 MB Idle RAM • Zero Cloud Daemons*

<br/>

[![PyPI version](https://img.shields.io/pypi/v/tapirus.svg?style=flat-square&logo=pypi)](https://pypi.org/project/tapirus/)
[![Python versions](https://img.shields.io/pypi/pyversions/tapirus.svg?style=flat-square&logo=python)](https://pypi.org/project/tapirus/)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg?style=flat-square)](LICENSE)
[![Memory Safety](https://img.shields.io/badge/memory--safety-100%25_Safe_Rust-brightgreen.svg?style=flat-square)](https://github.com/tapiruslab/TapirusDB)
[![Docs](https://img.shields.io/badge/docs-tapirusdb.com-2b3a7e.svg?style=flat-square)](https://tapirusdb.com/docs.html)

<br/>

</div>

---

## ⚡ Installation

Install from PyPI:

```bash
pip install tapirus
```

Or install from source:

```bash
pip install .
```

**Requirements:** Python 3.8+ (Supports CPython 3.8, 3.9, 3.10, 3.11, 3.12).

---

## 🚀 Quickstart

### 1. Relational SQL & ACID Transactions

```python
from tapirus import Connection

# Open or create a persistent database file (or ":memory:")
with Connection.open("production.tapir") as db:
    # Create structured table
    db.execute("""
        CREATE TABLE IF NOT EXISTS agents (
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            model TEXT NOT NULL,
            memory_mb REAL
        );
    """)

    # Insert records
    db.execute("INSERT INTO agents (id, name, model, memory_mb) VALUES (1, 'Echo-Agent', 'Phi-3-Mini', 3.8);")

    # Query rows as Python dictionaries
    rows = db.query("SELECT id, name, model, memory_mb FROM agents WHERE memory_mb < 100;")
    print("Agents:", rows)
```

---

### 2. Native HNSW Vector Similarity Search

```python
from tapirus import Connection

with Connection.open("production.tapir") as db:
    db.execute("""
        CREATE TABLE IF NOT EXISTS embeddings (
            doc_id INTEGER PRIMARY KEY,
            content TEXT,
            vector VECTOR(4)
        );
    """)

    # Insert embeddings
    db.execute("""
        INSERT INTO embeddings VALUES
        (1, 'Safe Systems Architecture', [0.12, 0.45, 0.88, -0.23]),
        (2, 'Quantum Neural Topology', [-0.42, 0.81, 0.15, 0.33]);
    """)

    # Perform nearest-neighbor similarity search
    results = db.query("""
        SELECT doc_id, content, VECTOR_COSINE(vector, [0.10, 0.40, 0.85, -0.20]) AS score
        FROM embeddings
        ORDER BY score DESC
        LIMIT 5;
    """)
    print("Top Matches:", results)
```

---

### 3. Knowledge Graph & openCypher GraphRAG

```python
from tapirus import Connection

with Connection.open("production.tapir") as db:
    # Create nodes and relationships
    db.graph_add_node(1, "Agent", '{"name":"Echo"}')
    db.graph_add_node(2, "Database", '{"name":"TapirusDB"}')
    db.graph_add_edge(1, 2, "USES", 1.0, '{"since":"2026"}')

    # openCypher pattern matching
    matches = db.graph_match("MATCH (a:Agent)-[r:USES]->(d:Database) RETURN a.name, d.name;")
    print("Graph Traversal:", matches)
```

---

### 4. Schemaless JSON Document Collections

```python
from tapirus import Connection

with Connection.open("production.tapir") as db:
    users = db.collection("users")

    # Insert nested JSON document
    doc_id = users.insert_one({
        "username": "faiz",
        "preferences": {"theme": "light", "telemetry": False},
        "tags": ["architect", "rust"]
    })

    # Find document by ID
    user = users.find_one({"_id": doc_id})
    print("User document:", user)
```

---

### 5. Encrypted Vault at Rest (ChaCha20-Poly1305 AEAD)

```python
from tapirus import Connection

# Open cryptographically authenticated container
with Connection.open("vault.tapir", passphrase="your-ultra-secure-passphrase") as db:
    db.execute("CREATE TABLE secrets (id INT PRIMARY KEY, token TEXT);")
    db.execute("INSERT INTO secrets VALUES (1, 'sk-agent-confidential-key');")
```

---

## 📜 License

The TapirusDB Python SDK is licensed under the [MIT License](LICENSE).  
The underlying TapirusDB core engine is licensed under [BUSL-1.1](https://github.com/tapiruslab/TapirusDB/blob/main/LICENSE).

For complete documentation, benchmarks, and architectural details, visit **[tapirusdb.com](https://tapirusdb.com)**.
