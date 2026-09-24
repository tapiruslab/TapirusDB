# TapirusDB Python SDK (`tapirus`)

Official Python client for **TapirusDB** — the 100% Safe-Rust Embedded Quad-Model Database & AI Agent Memory Engine.

---

## ⚡ Installation

Install from source or local checkout:
```bash
pip install .
```

Or build wheels for PyPI distribution:
```bash
python -m build
twine upload dist/*
```

---

## 🚀 Quickstart

```python
import tapirus

# 1. Connect to an encrypted single-file database
db = tapirus.connect("agent_memory.tapir", passphrase="super-secret-key")

# 2. Relational SQL & Transactions
db.execute("""
    CREATE TABLE documents (
        id INTEGER PRIMARY KEY,
        title TEXT,
        category TEXT,
        embedding VECTOR(3)
    );
""")

db.execute("""
    INSERT INTO documents VALUES 
    (1, 'Autonomous Drone Navigation', 'robotics', [0.99, 0.02, 0.01]),
    (2, 'Soil Moisture Sensor Mesh', 'iot', [0.12, 0.95, 0.05]);
""")

# 3. Query
rows = db.query("SELECT id, title, category FROM documents WHERE category = 'robotics';")
for row in rows:
    print(row)

# 4. In-Memory Mode
mem_db = tapirus.connect(":memory:")
mem_db.execute("CREATE TABLE kv (k TEXT PRIMARY KEY, v TEXT);")
```

---

## 🛡️ Architecture & Safety
- **Single-File `.tapir` Container**: Unified relational SQL, document JSON, HNSW vectors, and openCypher knowledge graphs in one file.
- **Hardware AES-256 GCM**: ChaCha20-Poly1305 AEAD with PBKDF2/SHA-256 salt verification.
- **Zero Cloud Latency**: Runs entirely in-process (`0.55 µs` pointer dereference) without external server daemons.
