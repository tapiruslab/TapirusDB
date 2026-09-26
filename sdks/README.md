# 🔌 TapirusDB Multi-Language SDK & FFI Ecosystem

Welcome to the official SDK and Foreign Function Interface (FFI) documentation for **TapirusDB** — the 100% Safe-Rust Embedded Quad-Model AI Database & Cognitive Memory Engine.

---

## 🏛 Architecture Overview

TapirusDB supports two high-performance integration topologies:

```
┌────────────────────────────────────────────────────────────────────────┐
│                        Application Layer                               │
├──────────────────┬─────────────────┬─────────────────┬─────────────────┤
│    Python        │     Node.js     │       Go        │       PHP       │
│  (C-FFI / ctypes)│  (IPC / HTTP)   │  (HTTP / RPC)   │  (HTTP / REST)  │
└────────┬─────────┴────────┬────────┴────────┬────────┴────────┬────────┘
         │                  │                 │                 │
         │ (Direct ABI)     │                 │                 │
         ▼                  │                 │                 │
┌──────────────────┐        │                 │                 │
│  libtapirus FFI  │        │                 │                 │
│  (.so/.dylib/.dll)       │                 │                 │
└────────┬─────────┘        │                 │                 │
         │                  ▼                 ▼                 ▼
         │           ┌──────────────────────────────────────────────────┐
         │           │             `tapirus serve` Daemon               │
         │           │    High-Performance Axum / HTTP / IPC Bridge     │
         │           └────────────────────────┬─────────────────────────┘
         │                                    │
         ▼                                    ▼
┌────────────────────────────────────────────────────────────────────────┐
│                   TapirusDB Core Engine (Safe Rust)                    │
│      Relational B+Tree • HNSW Vector • openCypher Graph • Documents    │
│                        Single `.tapir` Container                       │
└────────────────────────────────────────────────────────────────────────┘
```

| SDK / Language | Primary Protocol | Integration Type | Fallback Mode | Thread Safety |
| :--- | :--- | :--- | :--- | :--- |
| **Python** (`tapirus`) | C-ABI (ctypes) | Direct in-process shared library | Pure-Python memory emulator | Yes (Re-entrant) |
| **Node.js** (`@tapirus/sdk`) | IPC / HTTP | Microservice / Client | In-memory JS store | Yes |
| **Go** (`sdks/go`) | HTTP REST / IPC | High-concurrency client | N/A | Fully concurrent (`sync.Mutex`) |
| **PHP** (`tapirus/tapirus`) | HTTP REST | PSR-18 HTTP client | N/A | Request-scoped |
| **C / C++** (`include/tapirus.h`) | C99 ABI | Direct static/dynamic link | N/A | Thread-safe handles |

---

## 🛠 Compiling the Native C-ABI Shared Library (`libtapirus`)

For in-process native binding (Python, C, C++, or custom bindings), you need the compiled dynamic or static library.

### 1. Build from Source
```bash
# Clone the repository
git clone https://github.com/tapiruslab/TapirusDB.git
cd TapirusDB

# Build the C-FFI crate in Release mode
cargo build --release -p tapirus-ffi
```

### 2. Output Artifacts

| Platform | Shared Library (Dynamic) | Static Library | Header File |
| :--- | :--- | :--- | :--- |
| **Linux** | `target/release/libtapirus.so` | `target/release/libtapirus.a` | `include/tapirus.h` |
| **macOS** | `target/release/libtapirus.dylib` | `target/release/libtapirus.a` | `include/tapirus.h` |
| **Windows** | `target/release/tapirus.dll` | `target/release/tapirus.lib` | `include/tapirus.h` |

### 3. Installation to System Path (Optional)

```bash
# Linux / macOS
sudo cp target/release/libtapirus.* /usr/local/lib/
sudo cp include/tapirus.h /usr/local/include/
sudo ldconfig  # Linux only

# Or user directory
mkdir -p ~/.tapirus/lib
cp target/release/libtapirus.* ~/.tapirus/lib/
```

On Windows, copy `tapirus.dll` into your application directory or add its directory to your system `PATH`.

---

## 🐍 Python SDK (`sdks/python`)

The official Python client connects natively to the C-ABI engine via `ctypes`.

### Installation
```bash
pip install tapirus
# Or from local source:
cd sdks/python && pip install .
```

### Library Resolution
The Python SDK searches for the native shared library in the following order:
1. `TAPIRUS_LIB_PATH` environment variable (direct absolute path to `.so`/`.dll`/`.dylib`)
2. `TAPIRUS_LIB_DIR` directory override
3. Current working directory
4. Monorepo build directories (`target/release/`, `target/debug/`)
5. User library folder (`~/.tapirus/lib/`)
6. System paths (`/usr/local/lib`, `/usr/lib`)

If no compiled native library is found, the SDK gracefully falls back to an in-memory emulator for rapid development and testing.

### Usage Example
```python
import os
from tapirus import Connection

# Optional: Point to custom compiled library
# os.environ["TAPIRUS_LIB_PATH"] = "/path/to/libtapirus.so"

# Open single-file encrypted database
with Connection.open("production.tapir", passphrase="secret-agent-key") as db:
    # 1. SQL Relational
    db.execute("""
        CREATE TABLE IF NOT EXISTS agents (
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            status TEXT
        );
    """)
    db.execute("INSERT INTO agents VALUES (1, 'Nexus-9', 'active');")
    rows = db.query("SELECT * FROM agents;")
    print("Agents:", rows)

    # 2. Vector Similarity Search
    db.execute("""
        CREATE TABLE IF NOT EXISTS memory_vectors (
            id INTEGER PRIMARY KEY,
            content TEXT,
            embedding VECTOR(4)
        );
    """)
    db.execute("INSERT INTO memory_vectors VALUES (1, 'Sensor event', [0.1, 0.4, 0.9, -0.2]);")
    results = db.query("""
        SELECT id, content, VECTOR_COSINE(embedding, [0.1, 0.4, 0.8, -0.1]) AS sim
        FROM memory_vectors
        ORDER BY sim DESC LIMIT 1;
    """)
    print("Vector Search:", results)
```

---

## 🐹 Go SDK (`sdks/go`)

The official Go SDK provides high-performance client integration with `tapirus serve` over HTTP or local IPC.

### Installation
```bash
go get github.com/tapiruslab/TapirusDB/sdks/go
```

### Launching the Daemon
```bash
tapirus serve --bind 127.0.0.1:8080 --db production.tapir
```

### Usage Example
```go
package main

import (
	"context"
	"fmt"
	"log"

	tapirus "github.com/tapiruslab/TapirusDB/sdks/go"
)

func main() {
	ctx := context.Background()
	client := tapirus.NewClient(tapirus.Config{
		Endpoint: "http://127.0.0.1:8080",
	})

	// 1. Health & Status
	health, err := client.Health(ctx)
	if err != nil {
		log.Fatalf("Health check failed: %v", err)
	}
	fmt.Printf("Connected: %s (%s)\n", health.Version, health.Engine)

	// 2. SQL Query
	rows, err := client.Query(ctx, "SELECT id, name FROM agents WHERE status = 'active';")
	if err != nil {
		log.Fatal(err)
	}
	fmt.Println("Agents:", rows)

	// 3. HNSW Vector Nearest Neighbors
	matches, err := client.VectorSearch(ctx, "memory_vectors", []float32{0.1, 0.4, 0.8, -0.1}, 5)
	if err != nil {
		log.Fatal(err)
	}
	fmt.Println("Vector Matches:", matches)

	// 4. GraphRAG Traversal
	rag, err := client.GraphRAG(ctx, tapirus.GraphRAGRequest{
		Query: "Explain system telemetry",
		Seeds: 3,
		Hops:  2,
	})
	if err != nil {
		log.Fatal(err)
	}
	fmt.Println("Graph Context:", rag)
}
```

---

## 🐘 PHP SDK (`sdks/php`)

The official PHP client allows Laravel, Symfony, and vanilla PHP applications to interface with TapirusDB.

### Installation
```bash
composer require tapirus/tapirus
```

### Usage Example
```php
<?php

require_once __DIR__ . '/vendor/autoload.php';

use Tapirus\Client;

$client = new Client('http://127.0.0.1:8080');

// Execute SQL
$results = $client->query('SELECT * FROM agents WHERE status = ?;', ['active']);
print_r($results);

// Vector Search
$matches = $client->vectorSearch('embeddings', [0.12, 0.45, -0.67], 5);
print_r($matches);
```

---

## 🟨 Node.js SDK (`sdks/nodejs`)

The Node.js SDK allows JavaScript/TypeScript backends to interact with TapirusDB.

### Installation
```bash
npm install @tapirus/sdk
```

### Usage Example
```javascript
const tapirus = require('@tapirus/sdk');

const db = tapirus.open(':memory:');
db.execute("CREATE TABLE users (id INTEGER, name TEXT);");
db.execute("INSERT INTO users VALUES (1, 'Alice');");

const users = db.query("SELECT * FROM users;");
console.log(users);
```

---

## ⚡ C / C++ ABI Reference (`include/tapirus.h`)

For systems programming, IoT firmware, or custom language foreign function bindings, include `tapirus.h` and link against `libtapirus`:

```c
#include <stdio.h>
#include "tapirus.h"

int main() {
    // Open an encrypted database container
    TapirusConn* db = tapirus_open_encrypted("iot_device.tapir", "hardware-passphrase-2026");
    if (!db) {
        printf("Failed to open database\n");
        return 1;
    }

    // Execute schema migration
    tapirus_execute(db, "CREATE TABLE sensor_log (id INTEGER, temp REAL, ts INTEGER);");
    tapirus_execute(db, "INSERT INTO sensor_log VALUES (1, 23.4, 1790400000);");

    // Query JSON formatted results
    char* json = tapirus_query(db, "SELECT * FROM sensor_log;");
    if (json) {
        printf("Result: %s\n", json);
        tapirus_free_string(json);
    }

    // Close connection
    tapirus_close(db);
    return 0;
}
```

Compile with:
```bash
gcc main.c -Iinclude -Ltarget/release -ltapirus -o demo
./demo
```
