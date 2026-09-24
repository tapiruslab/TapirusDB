# @tapirus/tapirus 🦛

> **"One Engine. Four Models. Zero Data Sprawl."**  
> **TapirusDB in WebAssembly** — The Pure Safe-Rust Multi-Model Database & AI Agent Memory Engine for Web Browsers, Node.js, and Cloudflare Workers.  
> *Architected by **Ahmad Faiz***

[![npm version](https://img.shields.io/npm/v/tapirus.svg)](https://www.npmjs.com/package/tapirus)
[![License: BSL-1.1](https://img.shields.io/badge/License-BSL--1.1-blue.svg)](https://github.com/tapiruslab/TapirusDB/blob/main/LICENSE)
[![Wasm](https://img.shields.io/badge/WebAssembly-wasm32-purple.svg)](https://webassembly.org/)

## Features
- **Runs Client-Side in the Browser:** Zero server setup, zero backend database costs.
- **Quad-Model Architecture:** Embedded SQL relational tables, JSON collections, native vector similarity search, and property graphs.
- **AI Agent Memory & GraphRAG:** Built-in BM25 full-text indexing, associative memory, and Reciprocal Rank Fusion (RRF).
- **Ultra-Lightweight:** Loads fast and runs with `< 4 MB RAM` consumption.

## Quick Start (Browser / WebAssembly)

```javascript
import init, { TapirusWasm } from '@tapirus/db';

async function run() {
  // Initialize Wasm module
  await init();

  // Create in-memory database
  const db = new TapirusWasm();

  // Execute SQL
  db.execute("CREATE TABLE concepts (id INTEGER PRIMARY KEY, title TEXT);");
  db.execute("INSERT INTO concepts (id, title) VALUES (1, 'Neural Networks');");

  // Query SQL returning JSON
  const jsonResults = db.query_json("SELECT * FROM concepts;");
  console.log(JSON.parse(jsonResults));

  // Store in AI Agent Memory
  db.memory_remember("User prefers concise technical answers", 0.9, "preferences,tech");
  const memories = db.memory_recall_json("technical answers", 3);
  console.log("Recalled:", JSON.parse(memories));
}

run();
```

## Building from Source

To compile the WebAssembly package locally:

```bash
# Build for web browsers
wasm-pack build --target web --out-dir pkg

# Build for Node.js
wasm-pack build --target nodejs --out-dir pkg

# Build for bundlers (Webpack, Vite, Rollup)
wasm-pack build --target bundler --out-dir pkg
```

## License
Business Source License 1.1 (BSL-1.1). See the root [LICENSE](https://github.com/tapiruslab/TapirusDB/blob/master/LICENSE) for details.
