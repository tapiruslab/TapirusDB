<div align="center">

<img src="https://raw.githubusercontent.com/tapiruslab/TapirusDB/main/assets/icons/nodejs.svg" width="64" height="64" alt="Node.js Logo" />

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

## 🔰 Beginner's Step-by-Step Guide (Zero to Running in 60s)

> [!WARNING]
> **Common Beginner Mistake:** Do **NOT** paste JavaScript code (`import`, `const`, `console.log`) directly into your Windows PowerShell, Command Prompt, or Linux Bash terminal. The command line will error with `'import' is not recognized as a cmdlet`. Always write your code into a `.mjs` or `.js` file, and execute it using `node filename.mjs`!

### Step 1: Initialize your project folder
```powershell
mkdir my-tapirus-app
cd my-tapirus-app
npm init -y
npm install @tapirus/db
```

### Step 2: Create your script (`app.mjs`)
```javascript
import { TapirusDatabase } from '@tapirus/db';

const db = await TapirusDatabase.open('production.tapir');

// Create table
await db.execute('CREATE TABLE IF NOT EXISTS users (id INT, name TEXT);');
await db.execute('INSERT INTO users VALUES (1, "Faiz");');

// Query table
const rows = await db.query('SELECT * FROM users;');
console.log('Query result:', rows);
```

### Step 3: Run your script with Node.js
```powershell
node app.mjs
# Output: Query result: [ { id: 1, raw: "INSERT INTO users VALUES (1, 'Faiz')" } ]
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
