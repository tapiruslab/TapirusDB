<div align="center">

# TapirusDB Node.js & TypeScript SDK (`tapirus`)

### Official JavaScript & TypeScript Client for TapirusDB — Embedded Quad-Model AI Database & Memory Engine
**Relational SQL • HNSW Vector Search • openCypher Knowledge Graph • JSON Documents**  
*Single Encrypted `.tapir` Container • 100% Safe Rust Core • < 4 MB Idle RAM • Zero Cloud Daemons*

<br/>

[![npm](https://img.shields.io/npm/v/tapirus.svg?style=flat-square&logo=npm)](https://www.npmjs.com/package/tapirus)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg?style=flat-square)](LICENSE)
[![Node](https://img.shields.io/badge/node-%3E%3D18.0.0-green.svg?style=flat-square&logo=node.js)](https://nodejs.org)
[![TypeScript](https://img.shields.io/badge/TypeScript-Ready-blue.svg?style=flat-square&logo=typescript)](https://www.typescriptlang.org/)
[![Docs](https://img.shields.io/badge/docs-tapirusdb.com-2b3a7e.svg?style=flat-square)](https://tapirusdb.com/docs.html)

<br/>

</div>

---

## ⚡ Installation

Install from npm:

```bash
npm install tapirus
# Or using pnpm / yarn / bun
pnpm add tapirus
yarn add tapirus
bun add tapirus
```

**Requirements:** Node.js 18.0.0 or later (Full native support for Bun and Deno).

---

## 🚀 Quickstart

### 1. Relational SQL & ACID Transactions

```typescript
import { open } from 'tapirus';

// Open or create a persistent database file (or ":memory:")
const db = open('production.tapir');

// Create structured table
db.execute(`
  CREATE TABLE IF NOT EXISTS agents (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    model TEXT NOT NULL,
    memory_mb REAL
  );
`);

// Insert records with parameters
db.execute(
  'INSERT INTO agents (id, name, model, memory_mb) VALUES (?, ?, ?, ?);',
  [1, 'Echo-Agent', 'Phi-3-Mini', 3.8]
);

// Query rows as typed objects
const rows = db.query<{ id: number; name: string; model: string; memory_mb: number }>(
  'SELECT id, name, model, memory_mb FROM agents WHERE memory_mb < 100;'
);
console.log('Active Agents:', rows);
```

---

### 2. Native HNSW Vector Similarity Search

```typescript
import { open } from 'tapirus';

const db = open('production.tapir');

// Create table with dense vector embedding column
db.execute(`
  CREATE TABLE IF NOT EXISTS knowledge (
    id INTEGER PRIMARY KEY,
    content TEXT NOT NULL,
    embedding VECTOR(4)
  );
`);

// Insert embeddings
db.execute(`
  INSERT INTO knowledge VALUES
  (1, 'Safe Systems Architecture', [0.12, 0.45, 0.88, -0.23]),
  (2, 'Quantum Neural Topology', [-0.42, 0.81, 0.15, 0.33]);
`);

// Query top-K nearest neighbors using Cosine similarity
const queryVector = [0.10, 0.40, 0.85, -0.20];
const matches = db.query(`
  SELECT id, content, VECTOR_COSINE(embedding, ?) AS score
  FROM knowledge
  ORDER BY score DESC
  LIMIT 5;
`, [queryVector]);

console.log('Top Matches:', matches);
```

---

### 3. Knowledge Graph & openCypher GraphRAG

```typescript
import { open } from 'tapirus';

const db = open('production.tapir');

// Add property graph nodes & edges
db.graphAddNode(1, 'Agent', { name: 'Echo', role: 'Planner' });
db.graphAddNode(2, 'Database', { name: 'TapirusDB', version: '1.0.0' });
db.graphAddEdge(1, 2, 'USES', 1.0, { since: '2026' });

// openCypher pattern traversal
const results = db.graphMatch(
  'MATCH (a:Agent)-[r:USES]->(d:Database) RETURN a.name, d.name;'
);
console.log('Graph Relationships:', results);
```

---

### 4. Schemaless JSON Document Store

```typescript
import { open } from 'tapirus';

const db = open('production.tapir');
const users = db.collection('users');

// Insert nested JSON payload
const docId = users.insertOne({
  username: 'faiz',
  profile: { role: 'Architect', location: 'Kuala Lumpur' },
  models: ['phi-3', 'qwen-2.5']
});

// Retrieve by document ID
const user = users.findOne({ _id: docId });
console.log('User document:', user);
```

---

## 📜 License

The TapirusDB Node.js SDK is licensed under the [MIT License](LICENSE).  
The underlying TapirusDB core engine is licensed under [BUSL-1.1](https://github.com/tapiruslab/TapirusDB/blob/main/LICENSE).

For complete documentation, benchmarks, and architectural details, visit **[tapirusdb.com](https://tapirusdb.com)**.
