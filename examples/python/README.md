# 🦛 TapirusDB on Python

Integrate TapirusDB into Python applications with 100% Safe-Rust embedded speed.

---

## 🚀 Two Integration Options

### 1. High-Performance Native SDK (PyO3)
Built with PyO3 Rust extension modules for zero-overhead native bindings:

```python
import tapirus

# Connect to database file (or ":memory:")
conn = tapirus.connect("production.tapir")

# 1. Relational SQL Queries
conn.execute("CREATE TABLE IF NOT EXISTS agents (id INTEGER PRIMARY KEY, name TEXT);")
conn.execute("INSERT INTO agents VALUES (1, 'Hermes-Agent');")
for row in conn.query("SELECT * FROM agents;"):
    print("Agent:", row["id"], row["name"])

# 2. Schema-less Document Collections (MongoDB-style)
users = conn.collection("users")
users.insert_one({"name": "Alice", "role": "cryptographer"})
doc = users.find_one({"name": "Alice"})
print("Document:", doc)

# 3. Turnkey 1-Line Agent Memory & Prompt Synthesis
conn.remember("User prefers dark mode and concise code snippets")
prompt_context = conn.recall_prompt("user preferences", limit=2)
print("LLM Prompt Context:\n", prompt_context)
```

### 2. Standalone Ctypes FFI (Zero Rust Compiler Needed at Runtime)
Uses Python's standard `ctypes` library to communicate directly with `tapirus.dll` / `libtapirus.so`:

```bash
# Compile shared library
cargo build --release -p tapirus-ffi

# Run quickstart verification
python examples/python/quickstart.py

# Run orbital satellite telemetry demo
python examples/python/tapirus_demo.py
```

---

## 🛠️ Verification Test
Run the provided automated verification test:

```bash
python examples/python/quickstart.py
```
Output:
```text
Testing TapirusDB Python bindings (v1.0.1)...
Query Results: [{'id': 1, 'name': 'Iron Man', 'power': 105.0}]
Document Collection Verified: {'name': 'Jarvis', 'version': 'MK-85'}
Agent Memory Verified:
 ### [Retrieved Context]:
- Tony Stark built the arc reactor in a cave with scraps
ALL PYTHON TESTS PASSED SUCCESSFULLY!
```
