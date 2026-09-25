<div align="center">

# @tapirus/db

### Official TypeScript & JavaScript SDK for TapirusDB — Embedded Quad-Model AI Database & Memory Engine
**Relational SQL • HNSW Vector Search • openCypher Knowledge Graph • JSON Documents**  
*Single Encrypted `.tapir` Container • 100% Safe Rust Core • < 4 MB Idle RAM • Zero Cloud Daemons*

<br/>

[![npm](https://img.shields.io/npm/v/@tapirus/db.svg?style=flat-square&logo=npm)](https://www.npmjs.com/package/@tapirus/db)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg?style=flat-square)](LICENSE)
[![Node](https://img.shields.io/badge/node-%3E%3D18.0.0-green.svg?style=flat-square&logo=node.js)](https://nodejs.org)
[![TypeScript](https://img.shields.io/badge/TypeScript-Ready-blue.svg?style=flat-square&logo=typescript)](https://www.typescriptlang.org/)
[![Docs](https://img.shields.io/badge/docs-tapirusdb.com-2b3a7e.svg?style=flat-square)](https://tapirusdb.com/docs.html)

<br/>

</div>

---

## ⚡ Installation

```bash
npm install @tapirus/db
# Or using pnpm / yarn / bun
pnpm add @tapirus/db
yarn add @tapirus/db
bun add @tapirus/db
```

---

## 🚀 Quickstart

```typescript
import { TapirusDatabase } from '@tapirus/db';

async function main() {
  // 1. Open persistent single-file database container
  const db = await TapirusDatabase.open('production.tapir');

  // 2. Relational SQL Table
  await db.execute(`
    CREATE TABLE IF NOT EXISTS agents (
      id INT PRIMARY KEY,
      name TEXT NOT NULL,
      model TEXT NOT NULL
    );
  `);

  await db.execute('INSERT INTO agents VALUES (1, "Echo", "Phi-3-Mini");');

  // 3. Query records
  const agents = await db.query('SELECT * FROM agents;');
  console.log('Agents:', agents);

  // 4. Reactive CDC table subscription
  const sub = db.subscribe('agents', (change) => {
    console.log(`[CDC Event] ${change.op} on table ${change.table}:`, change.data);
  });

  // 5. Hybrid Search with Reciprocal Rank Fusion (RRF)
  const results = await db.hybridSearch({
    queryText: 'Autonomous cognitive memory',
    limit: 5,
    rrfK: 60
  });
  console.log('Hybrid Search Results:', results);

  // Cleanup
  sub.unsubscribe();
  db.close();
}

main();
```

---

## 📜 License

The `@tapirus/db` SDK is licensed under the [MIT License](LICENSE).  
The underlying TapirusDB core engine is licensed under [BUSL-1.1](https://github.com/tapiruslab/TapirusDB/blob/main/LICENSE).

For complete documentation, benchmarks, and architectural details, visit **[tapirusdb.com](https://tapirusdb.com)**.
